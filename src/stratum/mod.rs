pub mod connection;
pub mod pool;
pub mod protocol;

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time;

use self::connection::StratumStream;
use self::pool::{PoolConnection, PoolFailover};
use self::protocol::*;

#[derive(Debug, Clone, PartialEq)]
pub enum StratumState {
    Disconnected,
    Connecting,
    Connected,
    Authorized,
    Mining,
    Reconnecting,
}

pub struct ShareSubmission {
    pub job_id: String,
    pub nonce: u32,
    pub result: [u8; 32],
}

pub struct StratumStats {
    pub accepted: u64,
    pub rejected: u64,
}

pub struct StratumClient {
    pools: PoolFailover,
    rpc_id: String,
    seq: u64,
    retries: u32,
    retry_pause: u64,
    keepalive: bool,
}

impl StratumClient {
    pub fn new(pools: Vec<PoolConnection>, retries: u32, retry_pause: u64) -> Self {
        let keepalive = pools.first().map(|p| p.keepalive).unwrap_or(false);
        Self {
            pools: PoolFailover::new(pools),
            rpc_id: String::new(),
            seq: 1,
            retries,
            retry_pause,
            keepalive,
        }
    }

    /// Attach a reconnect signal for donation cycling.
    /// Returns the override-pool handle so the donation task can toggle the
    /// dev pool in and out.
    pub fn enable_donation_switching(
        &mut self,
        reconnect_signal: Arc<Notify>,
    ) -> Arc<Mutex<Option<PoolConnection>>> {
        self.pools.set_reconnect_signal(reconnect_signal);
        self.pools.override_handle()
    }

    /// Run the stratum client loop. Sends new jobs via the watch channel,
    /// receives share submissions via mpsc.
    pub async fn run(
        mut self,
        job_tx: watch::Sender<Option<MiningJob>>,
        mut submit_rx: mpsc::Receiver<ShareSubmission>,
        stats: Arc<tokio::sync::Mutex<StratumStats>>,
    ) {
        let mut retry_count = 0u32;
        let reconnect_signal = self.pools.reconnect_signal();

        loop {
            let pool = self.pools.current();
            match self
                .connect_and_mine(
                    &pool,
                    &job_tx,
                    &mut submit_rx,
                    &stats,
                    reconnect_signal.as_deref(),
                )
                .await
            {
                Ok(()) => {
                    log::info!("connection closed");
                    retry_count = 0;
                }
                Err(e) => {
                    log::warn!("disconnected: {}", e);
                    retry_count += 1;

                    if retry_count >= self.retries {
                        log::info!("max retries, switching pool");
                        self.pools.next();
                        retry_count = 0;
                    }
                }
            }

            log::info!("reconnecting in {}s...", self.retry_pause);
            time::sleep(Duration::from_secs(self.retry_pause)).await;
        }
    }

    async fn connect_and_mine(
        &mut self,
        pool: &PoolConnection,
        job_tx: &watch::Sender<Option<MiningJob>>,
        submit_rx: &mut mpsc::Receiver<ShareSubmission>,
        stats: &Arc<tokio::sync::Mutex<StratumStats>>,
        reconnect_signal: Option<&Notify>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = StratumStream::connect(pool).await?;
        log::info!("connected to \x1b[1m{}\x1b[0m", pool.address());

        self.cryptonote_session(
            &mut stream,
            pool,
            job_tx,
            submit_rx,
            stats,
            reconnect_signal,
        )
        .await
    }

    // ------------------------------------------------------------------
    // CryptoNote (RandomX) session.
    // ------------------------------------------------------------------
    async fn cryptonote_session(
        &mut self,
        stream: &mut StratumStream,
        pool: &PoolConnection,
        job_tx: &watch::Sender<Option<MiningJob>>,
        submit_rx: &mut mpsc::Receiver<ShareSubmission>,
        stats: &Arc<tokio::sync::Mutex<StratumStats>>,
        reconnect_signal: Option<&Notify>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Login
        let login = JsonRpcRequest::new(
            1,
            "login",
            LoginParams {
                login: pool.wallet.clone(),
                pass: pool.password.clone(),
                agent: AGENT_STRING.to_string(),
                rigid: pool.rig_id.clone(),
            },
        );

        let login_json = serde_json::to_string(&login)?;
        stream.write_line(&login_json).await?;

        // Read login response
        let response_line = stream.read_line().await?;
        let response: JsonRpcResponse = serde_json::from_str(&response_line)?;

        if let Some(error) = response.error {
            return Err(format!("Login failed: {}", error.message.unwrap_or_default()).into());
        }

        let result: LoginResult =
            serde_json::from_value(response.result.ok_or("Missing login result")?)?;

        self.rpc_id = result.id;
        log::info!("authorized");

        let job = MiningJob::from_params(&result.job, pool.nicehash, pool.algo)?;
        log::info!(
            "new job \x1b[1m{}\x1b[0m  diff {}  height {}",
            &job.job_id[..job.job_id.len().min(16)],
            u64::MAX / job.target.max(1),
            job.height
        );
        let _ = job_tx.send(Some(job));

        self.seq = 2;

        // Main loop: read pool messages + send submissions
        let keepalive_interval = if self.keepalive { 60 } else { u64::MAX };
        let mut keepalive_timer = time::interval(Duration::from_secs(keepalive_interval));
        keepalive_timer.tick().await; // consume first tick

        // `Notify::notified()` future is not Unpin; pin it once per iteration
        // via a helper closure. When no signal is wired, we race against a
        // future that never resolves.
        loop {
            let reconnect_fut = async {
                match reconnect_signal {
                    Some(n) => n.notified().await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                line = stream.read_line() => {
                    let line = line?;
                    let msg: JsonRpcResponse = serde_json::from_str(&line)?;
                    if self.handle_message(msg, pool, job_tx, stats).await? {
                        // Pool-driven redirect: end the session cleanly so the
                        // outer `run()` loop reconnects to the new endpoint.
                        return Ok(());
                    }
                }
                submission = submit_rx.recv() => {
                    if let Some(share) = submission {
                        self.submit_share(stream, &share).await?;
                    }
                }
                _ = keepalive_timer.tick() => {
                    if self.keepalive {
                        self.send_keepalive(stream).await?;
                    }
                }
                _ = reconnect_fut => {
                    log::info!(
                        "reconnect signal received, closing connection to {}",
                        pool.address()
                    );
                    // Graceful Ok — the outer `run()` loop will call
                    // `pools.current()` again, which picks up any donation
                    // override that was just installed.
                    return Ok(());
                }
            }
        }
    }

    /// Returns `Ok(true)` when the pool sent a `client.reconnect` directive
    /// and the session should end so the outer loop can reconnect to the new
    /// endpoint. `Ok(false)` for normal messages.
    async fn handle_message(
        &mut self,
        msg: JsonRpcResponse,
        pool: &PoolConnection,
        job_tx: &watch::Sender<Option<MiningJob>>,
        stats: &Arc<tokio::sync::Mutex<StratumStats>>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // New job notification
        if msg.method.as_deref() == Some("job") {
            if let Some(params) = msg.params {
                let job_params: JobParams = serde_json::from_value(params)?;
                let job = MiningJob::from_params(&job_params, pool.nicehash, pool.algo)?;
                log::info!(
                    "new job \x1b[1m{}\x1b[0m  diff {}  height {}",
                    &job.job_id[..job.job_id.len().min(16)],
                    u64::MAX / job.target.max(1),
                    job.height
                );
                let _ = job_tx.send(Some(job));
            }
        }
        // Pool-driven endpoint redirect (CryptoNote/MoneroOcean LB rotation).
        // Format: {"jsonrpc":"2.0","method":"client.reconnect",
        //         "params":["host", port, optional_wait_seconds]}
        else if msg.method.as_deref() == Some("client.reconnect") {
            if let Some(params) = msg.params {
                let arr = params.as_array().ok_or("client.reconnect params not array")?;
                let host = arr
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or("client.reconnect: missing host")?
                    .to_string();
                let port = arr
                    .get(1)
                    .and_then(|v| v.as_u64())
                    .ok_or("client.reconnect: missing port")? as u16;
                log::info!(
                    "\x1b[1;36m↻\x1b[0m pool redirect: {} -> {}:{}",
                    pool.address(),
                    host,
                    port
                );
                self.pools.set_redirect(host, port);
                return Ok(true);
            }
        }
        // Submit response
        else if let Some(id) = msg.id {
            if let Some(error) = msg.error {
                let mut s = stats.lock().await;
                s.rejected += 1;
                log::warn!(
                    "\x1b[1;31m✗\x1b[0m rejected ({}/{}) {}",
                    s.accepted,
                    s.accepted + s.rejected,
                    error.message.unwrap_or_default()
                );
            } else {
                let mut s = stats.lock().await;
                s.accepted += 1;
                log::info!(
                    "\x1b[1;32m✓\x1b[0m accepted ({}/{})",
                    s.accepted,
                    s.accepted + s.rejected,
                );
                let _ = id;
            }
        }

        Ok(false)
    }

    async fn submit_share(
        &mut self,
        stream: &mut StratumStream,
        share: &ShareSubmission,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let submit = JsonRpcRequest::new(
            self.seq,
            "submit",
            SubmitParams {
                id: self.rpc_id.clone(),
                job_id: share.job_id.clone(),
                nonce: hex::encode(share.nonce.to_le_bytes()),
                result: hex::encode(share.result),
            },
        );

        let json = serde_json::to_string(&submit)?;
        stream.write_line(&json).await?;

        log::debug!("Submitted share #{} for job {}", self.seq, share.job_id);
        self.seq += 1;

        Ok(())
    }

    async fn send_keepalive(
        &mut self,
        stream: &mut StratumStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let keepalive = JsonRpcRequest::new(
            self.seq,
            "keepalived",
            KeepaliveParams {
                id: self.rpc_id.clone(),
            },
        );

        let json = serde_json::to_string(&keepalive)?;
        stream.write_line(&json).await?;
        self.seq += 1;

        Ok(())
    }
}

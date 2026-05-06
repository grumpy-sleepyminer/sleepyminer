use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::algo::Algorithm;
use crate::config::PoolConfig;

/// Parsed pool connection details
#[derive(Debug, Clone)]
pub struct PoolConnection {
    pub host: String,
    pub port: u16,
    pub wallet: String,
    pub password: String,
    pub rig_id: Option<String>,
    pub nicehash: bool,
    pub tls: bool,
    pub keepalive: bool,
    pub algo: Algorithm,
}

impl PoolConnection {
    pub fn from_config(config: &PoolConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let (host, port) = parse_pool_url(&config.url)?;
        Ok(Self {
            host,
            port,
            wallet: config.wallet.clone(),
            password: config.password.clone(),
            rig_id: config.rig_id.clone(),
            nicehash: config.nicehash,
            tls: config.tls,
            keepalive: config.keepalive,
            algo: config.algo,
        })
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn parse_pool_url(url: &str) -> Result<(String, u16), Box<dyn std::error::Error>> {
    // Strip protocol prefix if present
    let url = url
        .strip_prefix("stratum+tcp://")
        .or_else(|| url.strip_prefix("stratum+ssl://"))
        .or_else(|| url.strip_prefix("stratum+tls://"))
        .unwrap_or(url);

    let parts: Vec<&str> = url.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid pool URL: {}. Expected host:port", url).into());
    }

    let port: u16 = parts[0]
        .parse()
        .map_err(|_| format!("Invalid port in pool URL: {}", parts[0]))?;
    let host = parts[1].to_string();

    Ok((host, port))
}

/// Manages failover between multiple pools.
///
/// Behavior:
/// - `current()` returns the currently-selected pool (or the donation override
///   pool, if one is active).
/// - `next()` cycles to the next pool in the user list on hard failover.
/// - The donation manager can install an override pool that `current()` will
///   return until the override is cleared.
/// - Pool-driven `client.reconnect` redirects rewrite the user pool's
///   host/port on the next `current()` call.
pub struct PoolFailover {
    pools: Vec<PoolConnection>,
    current: usize,
    /// Notified when the donation manager wants the stratum client to drop its
    /// current connection and re-read `current()`.
    reconnect_signal: Option<Arc<Notify>>,
    /// Shared override slot. When `Some`, `current()` returns this pool instead
    /// of the user-configured list. Used by the donation manager to time-slice
    /// into the dev pool without touching the user's config.
    override_pool: Arc<Mutex<Option<PoolConnection>>>,
    /// Pool-driven redirect target from a stratum `client.reconnect` message.
    /// When set, `current()` rewrites the user pool's host/port to this value
    /// (only when no donation override is active). Cleared on user-initiated
    /// failover (`next()`) or when the pool sends a fresh redirect.
    redirect: Arc<Mutex<Option<(String, u16)>>>,
}

impl PoolFailover {
    pub fn new(pools: Vec<PoolConnection>) -> Self {
        Self {
            pools,
            current: 0,
            reconnect_signal: None,
            override_pool: Arc::new(Mutex::new(None)),
            redirect: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach a reconnect signal. Used by the donation manager so it can kick
    /// the stratum loop when the dev/user pool flips.
    pub fn set_reconnect_signal(&mut self, reconnect_signal: Arc<Notify>) {
        self.reconnect_signal = Some(reconnect_signal);
    }

    pub fn reconnect_signal(&self) -> Option<Arc<Notify>> {
        self.reconnect_signal.clone()
    }

    /// Handle used by the donation manager to set/clear the override pool.
    /// Setting `Some(pool)` forces `current()` to return it; `None` reverts to
    /// the user-configured pool list. Caller is responsible for notifying the
    /// reconnect signal after flipping.
    pub fn override_handle(&self) -> Arc<Mutex<Option<PoolConnection>>> {
        self.override_pool.clone()
    }

    pub fn current(&mut self) -> PoolConnection {
        if let Some(ovr) = self.override_pool.lock().unwrap().clone() {
            // Donation override is active — go straight to the dev pool, no redirect.
            return ovr;
        }
        let mut pool = self.pools[self.current].clone();
        if let Some((host, port)) = self.redirect.lock().unwrap().clone() {
            pool.host = host;
            pool.port = port;
        }
        pool
    }

    /// Record a `client.reconnect` redirect target. The next `current()` call
    /// rewrites the user pool's host/port to this value.
    pub fn set_redirect(&self, host: String, port: u16) {
        *self.redirect.lock().unwrap() = Some((host, port));
    }

    /// Discard any active redirect (e.g. on hard pool failover).
    pub fn clear_redirect(&self) {
        *self.redirect.lock().unwrap() = None;
    }

    pub fn next(&mut self) -> PoolConnection {
        // An override is in force; cycling past it doesn't make sense — return
        // the override and let the donation manager clear it when ready.
        if let Some(ovr) = self.override_pool.lock().unwrap().clone() {
            return ovr;
        }
        // Hard failover discards any pool-driven redirect.
        self.clear_redirect();
        self.current = (self.current + 1) % self.pools.len();
        self.pools[self.current].clone()
    }

    pub fn reset(&mut self) {
        self.current = 0;
    }

    pub fn len(&self) -> usize {
        self.pools.len()
    }
}

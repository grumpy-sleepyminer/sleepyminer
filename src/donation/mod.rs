use rand::Rng;
use sha3::{Digest, Keccak256};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::config::PoolConfig;
use crate::stratum::pool::PoolConnection;

/// Dev donation wallet address (MoneroOcean)
const DEV_WALLET: &str = "82vnSSqeHAyhhbrHXtJkhFAEYUrCGgbdSQXb7rZBTxXSFZswAvoaBHfQLtK3QizSfzjUpgExRm5CMjPWyea41WvdUcaAz6m";

/// Dev donation pool
const DEV_POOL: &str = "gulf.moneroocean.stream:10128";

pub struct DonationManager {
    donate_level: u8,
    user_duration: Duration,
    dev_duration: Duration,
    state: DonationState,
    timer_start: Instant,
    current_duration: Duration,
    first_round: bool,
    dev_pool: PoolConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DonationState {
    UserMining,
    DevMining,
}

impl DonationManager {
    pub fn new(donate_level: u8, user_wallet: &str) -> Self {
        let level = donate_level.max(1) as u64;
        let user_mins = 100u64.saturating_sub(level);
        let dev_mins = level;

        // Generate donation ID from user wallet
        let donation_id = Self::generate_donation_id(user_wallet);

        let dev_pool = PoolConfig {
            url: DEV_POOL.to_string(),
            wallet: DEV_WALLET.to_string(),
            password: donation_id,
            rig_id: None,
            nicehash: false,
            tls: false,
            keepalive: true,
            enabled: true,
            algo: crate::algo::Algorithm::RandomX,
        };

        // Randomize first round duration
        let mut rng = rand::thread_rng();
        let first_user_duration =
            Duration::from_secs(user_mins * 60).mul_f64(rng.gen_range(0.5..1.5));

        Self {
            donate_level: donate_level.max(1),
            user_duration: Duration::from_secs(user_mins * 60),
            dev_duration: Duration::from_secs(dev_mins * 60),
            state: DonationState::UserMining,
            timer_start: Instant::now(),
            current_duration: first_user_duration,
            first_round: true,
            dev_pool,
        }
    }

    /// Check if it's time to switch pools. Returns Some(new_state) if switch needed.
    pub fn tick(&mut self) -> Option<DonationState> {
        if self.timer_start.elapsed() >= self.current_duration {
            let new_state = match self.state {
                DonationState::UserMining => {
                    self.current_duration = self.dev_duration;
                    DonationState::DevMining
                }
                DonationState::DevMining => {
                    self.current_duration = self.user_duration;
                    self.first_round = false;
                    DonationState::UserMining
                }
            };

            self.state = new_state.clone();
            self.timer_start = Instant::now();

            log::debug!(
                "Donation switch -> {:?} for {:.0}s",
                self.state,
                self.current_duration.as_secs_f64()
            );

            Some(new_state)
        } else {
            None
        }
    }

    pub fn state(&self) -> &DonationState {
        &self.state
    }

    pub fn dev_pool(&self) -> &PoolConfig {
        &self.dev_pool
    }

    pub fn is_donating(&self) -> bool {
        self.state == DonationState::DevMining
    }

    fn generate_donation_id(wallet: &str) -> String {
        let mut hasher = Keccak256::new();
        hasher.update(wallet.as_bytes());
        let result = hasher.finalize();
        hex::encode(&result[..16])
    }

    /// Build a `PoolConnection` for the dev pool using the same parse path as
    /// user-configured pools.
    fn dev_pool_connection(&self) -> Result<PoolConnection, Box<dyn std::error::Error>> {
        PoolConnection::from_config(&self.dev_pool)
    }
}

/// Background task: drive the user/dev cycle by toggling the override pool and
/// nudging the reconnect signal. Runs forever.
pub async fn run_donation_loop(
    mut manager: DonationManager,
    override_handle: Arc<Mutex<Option<PoolConnection>>>,
    reconnect_signal: Arc<Notify>,
) {
    let dev_pool = match manager.dev_pool_connection() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("donation: failed to build dev pool ({}), donations disabled", e);
            return;
        }
    };

    log::info!(
        "donation: time-slicing {}% of cycles to the dev pool",
        manager.donate_level
    );

    loop {
        let sleep_for = manager
            .current_duration
            .saturating_sub(manager.timer_start.elapsed());
        if sleep_for > Duration::from_millis(0) {
            tokio::time::sleep(sleep_for).await;
        }
        if let Some(new_state) = manager.tick() {
            match new_state {
                DonationState::DevMining => {
                    *override_handle.lock().unwrap() = Some(dev_pool.clone());
                    log::info!(
                        "\x1b[1;35m★\x1b[0m donation cycle START — mining to dev pool for {}s",
                        manager.dev_duration.as_secs()
                    );
                    reconnect_signal.notify_one();
                }
                DonationState::UserMining => {
                    *override_handle.lock().unwrap() = None;
                    log::info!(
                        "\x1b[1;32m✓\x1b[0m donation cycle END — resuming user pool for {}s",
                        manager.user_duration.as_secs()
                    );
                    reconnect_signal.notify_one();
                }
            }
        }
    }
}

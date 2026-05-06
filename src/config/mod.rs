pub mod cli;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::algo::Algorithm;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub pools: Vec<PoolConfig>,
    #[serde(default = "default_threads")]
    pub threads: Option<usize>,
    #[serde(default = "default_donate_level")]
    pub donate_level: u8,
    #[serde(default = "default_idle_threshold")]
    pub idle_threshold: u64,
    #[serde(default = "default_ramp_up_speed")]
    pub ramp_up_speed: u64,
    #[serde(default = "default_min_threads")]
    pub min_threads: usize,
    #[serde(default = "default_print_interval")]
    pub print_interval: u64,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default = "default_retry_pause")]
    pub retry_pause: u64,
    #[serde(default)]
    pub verbose: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub url: String,
    pub wallet: String,
    #[serde(default = "default_password")]
    pub password: String,
    pub rig_id: Option<String>,
    #[serde(default)]
    pub nicehash: bool,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub keepalive: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_algo")]
    pub algo: Algorithm,
}

fn default_algo() -> Algorithm {
    Algorithm::RandomX
}

fn default_threads() -> Option<usize> {
    None
}
fn default_donate_level() -> u8 {
    1
}
fn default_idle_threshold() -> u64 {
    120
}
fn default_ramp_up_speed() -> u64 {
    30
}
fn default_min_threads() -> usize {
    1
}
fn default_print_interval() -> u64 {
    60
}
fn default_retries() -> u32 {
    5
}
fn default_retry_pause() -> u64 {
    5
}
fn default_password() -> String {
    "x".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pools: vec![],
            threads: None,
            donate_level: 1,
            idle_threshold: 120,
            ramp_up_speed: 30,
            min_threads: 1,
            print_interval: 60,
            retries: 5,
            retry_pause: 5,
            verbose: false,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn default_path() -> PathBuf {
        dirs_path().join("config.json")
    }

    fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.pools.is_empty() {
            return Err("No pools configured. Add at least one pool.".into());
        }
        if self.donate_level < 1 {
            return Err("Minimum donation level is 1%.".into());
        }
        for pool in &self.pools {
            if pool.wallet.is_empty() {
                return Err("Pool wallet address cannot be empty.".into());
            }
            if pool.url.is_empty() {
                return Err("Pool URL cannot be empty.".into());
            }
        }
        Ok(())
    }

    pub fn max_threads(&self) -> usize {
        self.threads
            .unwrap_or_else(|| num_cpus::get().saturating_sub(1).max(1))
    }

    pub fn active_pools(&self) -> Vec<&PoolConfig> {
        self.pools.iter().filter(|p| p.enabled).collect()
    }
}

pub fn dirs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".sleepyminer")
}

pub fn generate_default_config(url: &str, wallet: &str, password: &str) -> Config {
    Config {
        pools: vec![PoolConfig {
            url: url.to_string(),
            wallet: wallet.to_string(),
            password: password.to_string(),
            rig_id: None,
            nicehash: false,
            tls: false,
            keepalive: true,
            enabled: true,
            algo: Algorithm::RandomX,
        }],
        ..Default::default()
    }
}

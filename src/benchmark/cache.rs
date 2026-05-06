//! Per-algorithm hashrate baseline cache.
//!
//! Saves measured hashrates to `~/.sleepyminer/benchmarks.json` so we don't
//! have to re-run them on every launch.
//!
//! Refresh policy:
//!   - Run benchmarks if no cache exists (first launch).
//!   - Run benchmarks if the cache is older than `MAX_AGE_DAYS` (~30 days).
//!   - Run benchmarks if the machine fingerprint changed (different hardware).
//!   - User can force a refresh via `sleepyminer benchmark`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::algo::Algorithm;

const SCHEMA_VERSION: u32 = 1;
const MAX_AGE_DAYS: u64 = 30;

#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkCache {
    pub version: u32,
    pub machine_id: String,
    /// Unix epoch seconds.
    pub last_updated: u64,
    /// Per-algo measurements.
    pub results: HashMap<String, AlgoResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AlgoResult {
    /// Throughput in H/s with the configured benchmark thread count.
    pub hashrate: f64,
    /// How many threads we used for that measurement.
    pub threads: usize,
    /// Convenience: H/s per thread (linear extrapolation point).
    pub hashrate_per_thread: f64,
    /// When this entry was last refreshed (epoch seconds).
    pub measured_at: u64,
}

impl BenchmarkCache {
    pub fn path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".sleepyminer")
            .join("benchmarks.json")
    }

    /// Load from disk, returning None if missing, malformed, or stale.
    pub fn load() -> Option<Self> {
        let path = Self::path();
        let content = std::fs::read_to_string(&path).ok()?;
        let cache: Self = serde_json::from_str(&content).ok()?;
        if cache.version != SCHEMA_VERSION {
            log::info!("benchmark cache schema version mismatch — discarding");
            return None;
        }
        if cache.machine_id != current_machine_id() {
            log::info!("benchmark cache for different machine — discarding");
            return None;
        }
        if cache.is_stale() {
            log::info!(
                "benchmark cache is older than {} days — will refresh",
                MAX_AGE_DAYS
            );
            return None;
        }
        Some(cache)
    }

    pub fn is_stale(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let age = now.saturating_sub(self.last_updated);
        age > MAX_AGE_DAYS * 24 * 60 * 60
    }

    /// Save to disk, creating ~/.sleepyminer/ if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn empty() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            version: SCHEMA_VERSION,
            machine_id: current_machine_id(),
            last_updated: now,
            results: HashMap::new(),
        }
    }

    pub fn get(&self, algo: Algorithm) -> Option<&AlgoResult> {
        self.results.get(algo_key(algo))
    }

    pub fn set(&mut self, algo: Algorithm, hashrate: f64, threads: usize) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let per_thread = if threads > 0 {
            hashrate / threads as f64
        } else {
            hashrate
        };
        self.results.insert(
            algo_key(algo).to_string(),
            AlgoResult {
                hashrate,
                threads,
                hashrate_per_thread: per_thread,
                measured_at: now,
            },
        );
        self.last_updated = now;
    }

    /// Convert to the (Algorithm, hashrate) tuple list.
    pub fn as_benchmarks(&self) -> Vec<(Algorithm, f64)> {
        let mut out = Vec::new();
        for &algo in &[Algorithm::RandomX] {
            if let Some(r) = self.get(algo) {
                out.push((algo, r.hashrate));
            }
        }
        out
    }

    /// True if the cache contains a measurement for every requested algo.
    pub fn covers_all(&self, algos: &[Algorithm]) -> bool {
        algos.iter().all(|a| self.get(*a).is_some())
    }
}

fn algo_key(a: Algorithm) -> &'static str {
    match a {
        Algorithm::RandomX => "randomx",
    }
}

/// Stable-ish identifier for "this machine" so we don't reuse benchmarks
/// across different hardware. macOS-only — we read the model identifier.
fn current_machine_id() -> String {
    // Fast and good enough: read sysctl hw.model.
    if let Ok(out) = std::process::Command::new("sysctl")
        .args(["-n", "hw.model"])
        .output()
    {
        if out.status.success() {
            let model = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !model.is_empty() {
                return format!("{}@{}cpus", model, num_cpus::get());
            }
        }
    }
    format!("unknown@{}cpus", num_cpus::get())
}

/// Throwaway helper for the runtime: how long until cache becomes stale.
pub fn _stale_after() -> Duration {
    Duration::from_secs(MAX_AGE_DAYS * 24 * 60 * 60)
}

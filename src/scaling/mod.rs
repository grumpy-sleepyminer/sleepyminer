pub mod activity;
pub mod cpu_load;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use self::cpu_load::CpuLoadMonitor;

/// Manages adaptive thread scaling based on user activity AND external CPU load.
///
/// Scale up only when:
///   - User is idle for ≥ `idle_threshold` seconds, AND
///   - External CPU usage (other processes) is below `cpu_ceiling` fraction
///
/// Scale down when either condition flips.
///
/// This prevents the miner from fighting with backups, builds, video calls,
/// or any other background work even when the user is away.
pub struct ActivityScaler {
    min_threads: usize,
    max_threads: usize,
    idle_threshold: f64,
    ramp_up_speed: f64,
    target_threads: Arc<AtomicUsize>,
    last_scale_up: Instant,
    cpu_monitor: CpuLoadMonitor,
    total_cores: usize,
    cpu_ceiling: f64, // max external load fraction (0..=1) before scaling down
}

impl ActivityScaler {
    pub fn new(
        min_threads: usize,
        max_threads: usize,
        idle_threshold: u64,
        ramp_up_speed: u64,
        target_threads: Arc<AtomicUsize>,
    ) -> Self {
        target_threads.store(min_threads, Ordering::SeqCst);
        Self {
            min_threads,
            max_threads,
            idle_threshold: idle_threshold as f64,
            ramp_up_speed: ramp_up_speed as f64,
            target_threads,
            last_scale_up: Instant::now(),
            cpu_monitor: CpuLoadMonitor::new(),
            total_cores: num_cpus::get(),
            cpu_ceiling: 0.35, // allow up to 35% external load before backing off
        }
    }

    /// Run the scaler loop. Call this from a tokio task.
    pub async fn run(&mut self) {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            interval.tick().await;
            self.tick();
        }
    }

    fn tick(&mut self) {
        let idle_secs = match activity::get_idle_seconds() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Failed to get idle time: {}", e);
                return;
            }
        };

        let current = self.target_threads.load(Ordering::SeqCst);

        // Estimate external CPU load: total busy minus our own mining threads.
        // Each active mining thread pegs ~1 core (100% of 1/total_cores).
        let external_load = if let Some(total_load) = self.cpu_monitor.sample() {
            let our_contribution = current as f64 / self.total_cores as f64;
            (total_load - our_contribution).max(0.0)
        } else {
            0.0 // first sample — treat as "no external load"
        };

        let user_active = idle_secs < self.idle_threshold;
        let system_busy = external_load > self.cpu_ceiling;

        if user_active || system_busy {
            // Yield — user is back or another process needs the CPU
            if current > self.min_threads {
                let reason = if user_active {
                    "user active"
                } else {
                    "system busy"
                };
                log::info!(
                    "↓ {} threads ({})",
                    self.min_threads,
                    reason,
                );
                self.target_threads
                    .store(self.min_threads, Ordering::SeqCst);
                self.last_scale_up = Instant::now();
            }
        } else {
            // Safe to ramp up — user away and system quiet
            if current < self.max_threads {
                let elapsed = self.last_scale_up.elapsed().as_secs_f64();
                if elapsed >= self.ramp_up_speed {
                    let new_threads = (current + 1).min(self.max_threads);
                    log::info!("↑ {} threads", new_threads);
                    self.target_threads.store(new_threads, Ordering::SeqCst);
                    self.last_scale_up = Instant::now();
                }
            }
        }
    }
}

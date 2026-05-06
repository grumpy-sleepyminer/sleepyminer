pub mod nonce;
pub mod worker;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use self::nonce::NonceCounter;
use self::worker::{SharedAlgoState, Worker};
use crate::algo::Algorithm;
use crate::stratum::protocol::MiningJob;
use crate::stratum::ShareSubmission;

pub struct MiningCoordinator {
    max_threads: usize,
    target_threads: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    hash_counts: Vec<Arc<AtomicU64>>,
    park_condvar: Arc<(Mutex<bool>, Condvar)>,
    total_hashes: Arc<AtomicU64>,
    start_time: Instant,
}

impl MiningCoordinator {
    pub fn new(max_threads: usize, target_threads: Arc<AtomicUsize>) -> Self {
        Self {
            max_threads,
            target_threads,
            running: Arc::new(AtomicBool::new(true)),
            hash_counts: Vec::new(),
            park_condvar: Arc::new((Mutex::new(false), Condvar::new())),
            total_hashes: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
        }
    }

    /// Start all worker threads and return channels for job distribution and share submission.
    pub fn start(
        &mut self,
        job_rx: watch::Receiver<Option<MiningJob>>,
        submit_tx: mpsc::Sender<ShareSubmission>,
    ) -> Vec<std::thread::JoinHandle<()>> {
        let nonce_counter = Arc::new(NonceCounter::new(0));
        let mut handles = Vec::new();

        // Shared per-algorithm state: all workers share one copy per (algo, seed)
        let shared_algo_state = Arc::new((
            Mutex::new(SharedAlgoState {
                state: None,
                algo: Algorithm::RandomX,
                seed_hash: [0u8; 32],
                initializing: false,
            }),
            Condvar::new(),
        ));

        for i in 0..self.max_threads {
            let job_rx = job_rx.clone();
            let submit_tx = submit_tx.clone();
            let nonce_counter = nonce_counter.clone();
            let target_threads = self.target_threads.clone();
            let park_condvar = self.park_condvar.clone();
            let running = self.running.clone();
            let hash_count = Arc::new(AtomicU64::new(0));
            let hash_count_clone = hash_count.clone();
            let _total_hashes = self.total_hashes.clone();
            let shared_algo_state = shared_algo_state.clone();

            self.hash_counts.push(hash_count);

            let handle = std::thread::Builder::new()
                .name(format!("miner-{}", i))
                .spawn(move || {
                    Worker::run(
                        i,
                        job_rx,
                        submit_tx,
                        nonce_counter,
                        target_threads,
                        park_condvar,
                        running,
                        hash_count_clone,
                        shared_algo_state,
                    );
                })
                .expect("Failed to spawn worker thread");

            handles.push(handle);
        }

        log::info!("{} workers ready", self.max_threads);
        handles
    }

    /// Wake up any parked workers (call after changing target_threads)
    pub fn wake_workers(&self) {
        let (_, cvar) = &*self.park_condvar;
        cvar.notify_all();
    }

    /// Get current hashrate (hashes per second) over a sampling window.
    pub fn hashrate(&self) -> f64 {
        let total: u64 = self
            .hash_counts
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .sum();
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            total as f64 / elapsed
        } else {
            0.0
        }
    }

    /// Get total hashes computed
    pub fn total_hashes(&self) -> u64 {
        self.hash_counts
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .sum()
    }

    /// Get number of currently active (non-parked) threads
    pub fn active_threads(&self) -> usize {
        self.target_threads.load(Ordering::SeqCst)
    }

    /// Stop all workers
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.wake_workers();
    }
}

/// Print mining status to stdout
pub fn print_status(coordinator: &MiningCoordinator, accepted: u64, rejected: u64) {
    let hashrate = coordinator.hashrate();
    let active = coordinator.active_threads();
    let total = coordinator.total_hashes();

    let (rate_str, unit) = if hashrate >= 1_000_000.0 {
        (format!("{:.2}", hashrate / 1_000_000.0), "MH/s")
    } else if hashrate >= 1000.0 {
        (format!("{:.2}", hashrate / 1000.0), "kH/s")
    } else {
        (format!("{:.1}", hashrate), "H/s")
    };
    log::info!(
        "\x1b[1;36m{} {}\x1b[0m  threads {}/{}  \x1b[32m{} accepted\x1b[0m  {} rejected",
        rate_str,
        unit,
        active,
        coordinator.max_threads,
        accepted,
        rejected,
    );
}

pub mod cache;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::algo::Algorithm;
use crate::randomx::hash::RandomXHasher;

/// Result of a single benchmark run
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub threads: usize,
    pub hashrate: f64,
    pub duration: Duration,
    pub hashes: u64,
}

/// Run a RandomX benchmark with the specified number of threads.
///
/// Builds a single shared cache+dataset and hands every worker a hasher
/// backed by that `Arc`. Spawning N independent hashers here would duplicate
/// RandomX's ~2 GB dataset per thread.
pub fn run_benchmark(
    seed_hash: &[u8; 32],
    threads: usize,
    duration: Duration,
) -> Result<BenchmarkResult, Box<dyn std::error::Error>> {
    let running = Arc::new(AtomicBool::new(true));
    let total_hashes = Arc::new(AtomicU64::new(0));
    let shared = crate::randomx::hash::RandomXState::new(seed_hash, threads)?;

    let mut handles = Vec::new();
    let start = Instant::now();

    for _ in 0..threads {
        let running = running.clone();
        let total_hashes = total_hashes.clone();
        let state = shared.clone();

        let handle = std::thread::spawn(move || {
            let mut hasher = RandomXHasher::with_state(state);
            let mut nonce = 0u32;
            let input = vec![0u8; 76];

            while running.load(Ordering::Relaxed) {
                let mut data = input.clone();
                data[39..43].copy_from_slice(&nonce.to_le_bytes());
                let _ = hasher.hash(&data);
                total_hashes.fetch_add(1, Ordering::Relaxed);
                nonce += 1;
            }
        });

        handles.push(handle);
    }

    std::thread::sleep(duration);
    running.store(false, Ordering::SeqCst);

    for handle in handles {
        let _ = handle.join();
    }

    let elapsed = start.elapsed();
    let hashes = total_hashes.load(Ordering::Relaxed);
    let hashrate = hashes as f64 / elapsed.as_secs_f64();

    Ok(BenchmarkResult {
        threads,
        hashrate,
        duration: elapsed,
        hashes,
    })
}

/// Run a per-algorithm hashrate benchmark for the given duration.
pub fn benchmark_algo(
    algo: Algorithm,
    threads: usize,
    duration: Duration,
) -> Result<BenchmarkResult, Box<dyn std::error::Error>> {
    let running = Arc::new(AtomicBool::new(true));
    let total_hashes = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    // Fixed dummy seed so results are comparable across runs.
    let seed = [0u8; 32];

    // RandomX: build ONE shared Argon2d cache+dataset (~2 GB) and hand every
    // thread a hasher backed by that Arc.
    let shared_randomx = crate::randomx::hash::RandomXState::new(&seed, threads)?;

    let start = Instant::now();

    for _ in 0..threads {
        let running = running.clone();
        let total_hashes = total_hashes.clone();
        let rx_state = shared_randomx.clone();

        let handle = std::thread::spawn(move || {
            let mut input = vec![0u8; 80];
            let mut nonce = 0u32;

            match algo {
                Algorithm::RandomX => {
                    let mut hasher = RandomXHasher::with_state(rx_state);
                    while running.load(Ordering::Relaxed) {
                        input[76..80].copy_from_slice(&nonce.to_le_bytes());
                        let _ = hasher.hash(&input);
                        total_hashes.fetch_add(1, Ordering::Relaxed);
                        nonce = nonce.wrapping_add(1);
                    }
                }
            }
        });
        handles.push(handle);
    }

    std::thread::sleep(duration);
    running.store(false, Ordering::SeqCst);
    for h in handles {
        let _ = h.join();
    }

    let elapsed = start.elapsed();
    let hashes = total_hashes.load(Ordering::Relaxed);
    let hashrate = hashes as f64 / elapsed.as_secs_f64();
    Ok(BenchmarkResult {
        threads,
        hashrate,
        duration: elapsed,
        hashes,
    })
}

/// Find optimal thread count by sweeping from 1 to max_threads.
pub fn find_optimal_threads(
    seed_hash: &[u8; 32],
    max_threads: usize,
    duration_per_test: Duration,
) -> Result<(usize, Vec<BenchmarkResult>), Box<dyn std::error::Error>> {
    let mut results = Vec::new();
    let mut best_threads = 1;
    let mut best_hashrate = 0.0;

    println!(
        "Starting benchmark sweep (1 to {} threads)...\n",
        max_threads
    );

    // Build the RandomX dataset once and reuse it across every thread-count
    // sweep — otherwise this loop spends ~10s per step on Argon2d+dataset
    // init and the total benchmark stretches to 2+ minutes.
    print!("  Initializing RandomX dataset... ");
    let init_t0 = Instant::now();
    let shared = crate::randomx::hash::RandomXState::new(seed_hash, max_threads)?;
    println!("done in {:.1}s\n", init_t0.elapsed().as_secs_f64());

    for threads in 1..=max_threads {
        print!("  Testing {} thread(s)... ", threads);
        let result = run_benchmark_shared(&shared, threads, duration_per_test);
        println!("{:.1} H/s", result.hashrate);

        if result.hashrate > best_hashrate {
            best_hashrate = result.hashrate;
            best_threads = threads;
        }

        results.push(result);
    }

    println!(
        "\nOptimal: {} threads @ {:.1} H/s",
        best_threads, best_hashrate
    );

    Ok((best_threads, results))
}

/// Hash loop against an already-built shared RandomX state.
fn run_benchmark_shared(
    shared: &Arc<crate::randomx::hash::RandomXState>,
    threads: usize,
    duration: Duration,
) -> BenchmarkResult {
    let running = Arc::new(AtomicBool::new(true));
    let total_hashes = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    let start = Instant::now();

    for _ in 0..threads {
        let running = running.clone();
        let total_hashes = total_hashes.clone();
        let state = shared.clone();

        let handle = std::thread::spawn(move || {
            let mut hasher = RandomXHasher::with_state(state);
            let mut nonce = 0u32;
            let input = vec![0u8; 76];
            while running.load(Ordering::Relaxed) {
                let mut data = input.clone();
                data[39..43].copy_from_slice(&nonce.to_le_bytes());
                let _ = hasher.hash(&data);
                total_hashes.fetch_add(1, Ordering::Relaxed);
                nonce += 1;
            }
        });
        handles.push(handle);
    }

    std::thread::sleep(duration);
    running.store(false, Ordering::SeqCst);
    for h in handles {
        let _ = h.join();
    }

    let elapsed = start.elapsed();
    let hashes = total_hashes.load(Ordering::Relaxed);
    let hashrate = hashes as f64 / elapsed.as_secs_f64();
    BenchmarkResult {
        threads,
        hashrate,
        duration: elapsed,
        hashes,
    }
}

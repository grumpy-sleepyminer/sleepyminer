use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use tokio::sync::{mpsc, watch};

use super::nonce::NonceCounter;
use crate::algo::{AlgoState, Algorithm, Hasher};
use crate::stratum::protocol::MiningJob;
use crate::stratum::ShareSubmission;

/// Shared per-algorithm state holder.
///
/// Workers coordinate through this so that only one copy of expensive shared
/// state (e.g. RandomX's multi-GB cache+dataset) is built per (algorithm, seed).
pub struct SharedAlgoState {
    pub state: Option<Arc<AlgoState>>,
    pub algo: Algorithm,
    pub seed_hash: [u8; 32],
    /// Set to true while a worker is initializing the state.
    pub initializing: bool,
}

/// A single mining worker thread.
pub struct Worker {
    pub thread_id: usize,
    pub hash_count: Arc<AtomicU64>,
}

impl Worker {
    /// Run the worker loop. This blocks the current thread.
    pub fn run(
        thread_id: usize,
        mut job_rx: watch::Receiver<Option<MiningJob>>,
        submit_tx: mpsc::Sender<ShareSubmission>,
        nonce_counter: Arc<NonceCounter>,
        target_threads: Arc<AtomicUsize>,
        park_condvar: Arc<(Mutex<bool>, Condvar)>,
        running: Arc<AtomicBool>,
        hash_count: Arc<AtomicU64>,
        shared_algo_state: Arc<(Mutex<SharedAlgoState>, Condvar)>,
    ) {
        log::debug!("Worker {} starting", thread_id);

        // Set thread QoS + weak affinity hint to keep miners on P-core clusters
        #[cfg(target_os = "macos")]
        unsafe {
            extern "C" {
                fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
                fn pthread_mach_thread_np(thread: libc::pthread_t) -> u32;
                fn thread_policy_set(
                    thread: u32,
                    flavor: i32,
                    policy_info: *const i32,
                    count: u32,
                ) -> i32;
            }

            // Mining threads should prefer P-cores
            pthread_set_qos_class_self_np(0x19, 0); // QOS_CLASS_USER_INITIATED

            // Weak hint: macOS uses this as an L2/cluster locality tag (often ignored, but harmless)
            // M4 Pro: 5 P-cores per cluster (as per your earlier assumption)
            #[allow(non_upper_case_globals)]
            const THREAD_AFFINITY_POLICY: i32 = 4;
            #[allow(non_upper_case_globals)]
            const THREAD_AFFINITY_POLICY_COUNT: u32 = 1;

            let tag = (thread_id / 5) + 1;
            let policy = [tag as i32];
            let mach_port = pthread_mach_thread_np(libc::pthread_self());
            thread_policy_set(
                mach_port,
                THREAD_AFFINITY_POLICY,
                policy.as_ptr(),
                THREAD_AFFINITY_POLICY_COUNT,
            );
        }

        // Each worker gets its own hasher (owns VM, but shares cache+dataset)
        let mut hasher: Option<Box<dyn Hasher>> = None;
        let mut current_seed = [0u8; 32];
        let mut current_algo: Option<Algorithm> = None;

        while running.load(Ordering::Relaxed) {
            // Check if we should be parked
            let target = target_threads.load(Ordering::SeqCst);
            if thread_id >= target {
                log::debug!("Worker {} parking (target_threads={})", thread_id, target);
                let (lock, cvar) = &*park_condvar;
                let mut parked = lock.lock().unwrap();
                while thread_id >= target_threads.load(Ordering::SeqCst)
                    && running.load(Ordering::Relaxed)
                {
                    parked = cvar.wait(parked).unwrap();
                }
                log::debug!("Worker {} unparked", thread_id);
                continue;
            }

            // Get current job and mark as seen
            let job = {
                let job_ref = job_rx.borrow_and_update();
                job_ref.clone()
            };

            let job = match job {
                Some(j) => j,
                None => {
                    // No job yet, wait for one
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(handle) = rt {
                        // We're in a tokio context
                        let mut rx = job_rx.clone();
                        let _ = handle.block_on(rx.changed());
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    continue;
                }
            };

            // Initialize or reinitialize hasher if algo or seed changed
            if hasher.is_none() || current_algo != Some(job.algo) || current_seed != job.seed_hash {
                log::debug!(
                    "worker {} init {} seed {}",
                    thread_id,
                    job.algo.name(),
                    hex::encode(&job.seed_hash[..8])
                );

                let state = Self::get_or_init_shared_state(
                    thread_id,
                    job.algo,
                    &job.seed_hash,
                    &shared_algo_state,
                    &running,
                    target_threads.load(Ordering::Relaxed),
                );

                match state.and_then(|s| s.create_hasher().ok().map(|h| (s, h))) {
                    Some((_s, h)) => {
                        hasher = Some(h);
                        current_seed = job.seed_hash;
                        current_algo = Some(job.algo);
                    }
                    None => {
                        // Initialization failed or shutting down
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        continue;
                    }
                }
            }

            let job_id = job.job_id.clone();

            let h = hasher.as_mut().unwrap();
            // Reuse input buffer to avoid per-iteration Vec allocation
            let mut input = job.blob.clone();
            if let Some(ref extra) = job.extra_nonce {
                let offset = job.nonce_offset + 4 - extra.len();
                if offset + extra.len() <= input.len() {
                    input[offset..offset + extra.len()].copy_from_slice(extra);
                }
            }

            let mut iterations = 0usize;
            // Hoist bounds checks out of hot loop for better pipeline utilization
            let nonce_off = job.nonce_offset;
            let blob_len = input.len();

            // NiceHash mode: the pool reserves the top byte of the 4-byte nonce
            // field as an "extraNonce" fixed byte, shipped baked into the job blob.
            // Miners must preserve it and only vary the lower 3 bytes.
            // 3 bytes = 16.77M nonces per job — plenty for seconds of hashing.
            let nicehash_mode = job.nicehash;
            let fixed_top_byte = if nicehash_mode && nonce_off + 4 <= blob_len {
                input[nonce_off + 3]
            } else {
                0
            };

            // Reduce watch polling overhead: call has_changed less frequently
            let mut next_job_check = 0usize;

            loop {
                if thread_id >= target_threads.load(Ordering::Relaxed)
                    || !running.load(Ordering::Relaxed)
                {
                    break;
                }

                // Check job changes only occasionally
                if iterations >= next_job_check {
                    // If has_changed() isn't available, conservatively break
                    if job_rx.has_changed().unwrap_or(true) {
                        break;
                    }
                    // Re-check every 256 nonces (tunable)
                    next_job_check = iterations + 256;
                }

                let raw_nonce = nonce_counter.next();
                // In NiceHash mode the top byte is reserved by the pool — mask it off
                // so we never clobber it and our submission uses the same full u32
                // the pool expects (fixed_top_byte in MSB + our 3 bytes in LSB).
                let nonce = if nicehash_mode {
                    (raw_nonce & 0x00FF_FFFF) | ((fixed_top_byte as u32) << 24)
                } else {
                    raw_nonce
                };

                // Avoid slice creation/copy_from_slice overhead; write 4 bytes directly.
                if nonce_off + 4 <= blob_len {
                    let b = nonce.to_le_bytes();
                    input[nonce_off] = b[0];
                    input[nonce_off + 1] = b[1];
                    input[nonce_off + 2] = b[2];
                    input[nonce_off + 3] = b[3];
                }

                iterations = iterations.wrapping_add(1);

                // Compute hash
                let result = match h.hash(&input) {
                    Ok(r) => r,
                    Err(e) => {
                        // Avoid spamming logs in hot path; correctness unchanged
                        log::error!("Hash error: {}", e);
                        continue;
                    }
                };

                hash_count.fetch_add(1, Ordering::Relaxed);

                // Check against target
                let hash_val = u64::from_le_bytes(result[24..32].try_into().unwrap());
                if hash_val < job.target {
                    log::info!("🌠 share found  nonce {:08x}", nonce);
                    let _ = submit_tx.blocking_send(ShareSubmission {
                        job_id: job_id.clone(),
                        nonce,
                        result,
                    });
                }
            }
        }

        log::debug!("Worker {} stopped", thread_id);
    }

    /// Get the shared algorithm state, initializing it if this is the first
    /// worker to encounter a new (algo, seed) pair. Other workers wait for
    /// initialization to complete.
    fn get_or_init_shared_state(
        thread_id: usize,
        algo: Algorithm,
        seed_hash: &[u8; 32],
        shared: &Arc<(Mutex<SharedAlgoState>, Condvar)>,
        running: &Arc<AtomicBool>,
        init_threads: usize,
    ) -> Option<Arc<AlgoState>> {
        let (lock, cvar) = &**shared;

        // First, check if state is already available or if we need to initialize
        {
            let mut guard = lock.lock().unwrap();

            // State already exists for this (algo, seed) - just clone the Arc
            if guard.state.is_some() && guard.algo == algo && guard.seed_hash == *seed_hash {
                log::debug!("worker {} reusing shared {} state", thread_id, algo.name());
                return guard.state.clone();
            }

            // Another worker is already initializing - wait for it
            if guard.initializing {
                log::info!(
                    "Worker {} waiting for shared {} state initialization",
                    thread_id,
                    algo.name()
                );
                while guard.initializing && running.load(Ordering::Relaxed) {
                    guard = cvar.wait(guard).unwrap();
                }
                if !running.load(Ordering::Relaxed) {
                    return None;
                }
                // After waking, check if the state matches our (algo, seed)
                if guard.state.is_some() && guard.algo == algo && guard.seed_hash == *seed_hash {
                    return guard.state.clone();
                }
                // Algo or seed changed again while we waited - fall through to initialize
            }

            // We are the first worker to notice this (algo, seed) - claim init
            guard.initializing = true;
            // Drop old state before creating new one to free memory
            guard.state = None;
        }

        // Initialize outside the lock (RandomX init takes minutes)
        log::debug!(
            "worker {} initializing {} state",
            thread_id,
            algo.name()
        );
        let result = AlgoState::for_algo(algo, seed_hash, init_threads).map(Arc::new);

        let mut guard = lock.lock().unwrap();
        guard.initializing = false;

        match result {
            Ok(state) => {
                guard.algo = algo;
                guard.seed_hash = *seed_hash;
                guard.state = Some(state.clone());
                cvar.notify_all();
                log::debug!("worker {} shared {} state ready", thread_id, algo.name());
                Some(state)
            }
            Err(e) => {
                log::error!(
                    "Worker {} failed to init shared {} state: {}",
                    thread_id,
                    algo.name(),
                    e
                );
                cvar.notify_all();
                None
            }
        }
    }
}

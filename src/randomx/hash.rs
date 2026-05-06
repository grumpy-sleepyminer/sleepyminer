use super::ffi;
use crate::algo::{Algorithm, Hasher};
use std::sync::Arc;

pub const RANDOMX_HASH_SIZE: usize = 32;

/// Shared RandomX state (cache + dataset).
/// Created once per seed hash, shared across all worker threads via Arc.
pub struct RandomXState {
    cache: *mut ffi::RandomxCache,
    dataset: *mut ffi::RandomxDataset,
    flags: ffi::RandomxFlags,
}

// These pointers are safe to share - the C library handles thread safety
// for read-only access to cache/dataset after initialization.
unsafe impl Send for RandomXState {}
unsafe impl Sync for RandomXState {}

impl RandomXState {
    /// Build a RandomX cache + dataset.
    ///
    /// `init_threads` controls how many threads parallelize the 2GB dataset
    /// build. Pass the user's chosen mining-thread count (so the cores that
    /// will mine also warm the dataset). Internally clamped to
    /// `[1, num_cpus::get()]`.
    pub fn new(
        seed_hash: &[u8; 32],
        init_threads: usize,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        // macOS requires root privileges for large pages. Dropping LARGE_PAGES prevents
        // randomx_alloc_cache from failing. HARD_AES | FULL_MEM | JIT is optimal.
        let flags = unsafe { ffi::randomx_get_flags() }
            | ffi::RANDOMX_FLAG_FULL_MEM
            | ffi::RANDOMX_FLAG_JIT;

        log::debug!("RandomX flags: 0x{:x}", flags);

        let cache = unsafe { ffi::randomx_alloc_cache(flags) };
        if cache.is_null() {
            return Err("Failed to allocate RandomX cache".into());
        }

        log::info!("initializing RandomX cache...");
        let t = std::time::Instant::now();
        unsafe {
            ffi::randomx_init_cache(cache, seed_hash.as_ptr() as *const _, seed_hash.len());
        }
        log::info!("cache ready ({:.1}s)", t.elapsed().as_secs_f64());

        // Allocate and initialize dataset (2GB, parallelized)
        let dataset = unsafe { ffi::randomx_alloc_dataset(flags) };
        if dataset.is_null() {
            unsafe {
                ffi::randomx_release_cache(cache);
            }
            return Err("Failed to allocate RandomX dataset".into());
        }

        let item_count = unsafe { ffi::randomx_dataset_item_count() };
        // Match the user's mining-thread count so the same cores that will mine
        // also build the dataset. Clamp to physical core count to be safe.
        let num_threads = (init_threads.max(1).min(num_cpus::get())) as u64;
        let items_per_thread = item_count / num_threads;

        log::info!("initializing RandomX dataset (2 GB, {} threads)...", num_threads);
        let t = std::time::Instant::now();

        // Parallel dataset init using scoped threads
        let cache_ptr = cache as usize;
        let dataset_ptr = dataset as usize;

        std::thread::scope(|s| {
            for tid in 0..num_threads {
                let start = tid * items_per_thread;
                let count = if tid == num_threads - 1 {
                    item_count - start
                } else {
                    items_per_thread
                };
                let cp = cache_ptr;
                let dp = dataset_ptr;

                s.spawn(move || {
                    // Elevate QoS for dataset init threads to ensure fast P-core scheduling
                    #[cfg(target_os = "macos")]
                    unsafe {
                        extern "C" {
                            fn pthread_set_qos_class_self_np(
                                qos_class: u32,
                                relative_priority: i32,
                            ) -> i32;
                        }
                        pthread_set_qos_class_self_np(0x21, 0);
                    }
                    unsafe {
                        ffi::randomx_init_dataset(
                            dp as *mut ffi::RandomxDataset,
                            cp as *mut ffi::RandomxCache,
                            start,
                            count,
                        );
                    }
                });
            }
        });

        log::info!("dataset ready ({:.1}s)", t.elapsed().as_secs_f64());

        Ok(Arc::new(Self {
            cache,
            dataset,
            flags,
        }))
    }
}

impl Drop for RandomXState {
    fn drop(&mut self) {
        unsafe {
            if !self.dataset.is_null() {
                ffi::randomx_release_dataset(self.dataset);
            }
            if !self.cache.is_null() {
                ffi::randomx_release_cache(self.cache);
            }
        }
    }
}

/// Per-thread RandomX hasher. Each worker owns one of these.
/// Shares the cache+dataset via Arc<RandomXState>.
pub struct RandomXHasher {
    state: Arc<RandomXState>,
    vm: *mut ffi::RandomxVm,
}

unsafe impl Send for RandomXHasher {}

impl RandomXHasher {
    pub fn with_state(state: Arc<RandomXState>) -> Self {
        let vm = unsafe { ffi::randomx_create_vm(state.flags, state.cache, state.dataset) };
        assert!(!vm.is_null(), "Failed to create RandomX VM");

        Self { state, vm }
    }

    pub fn new(
        seed_hash: &[u8; 32],
        init_threads: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let state = RandomXState::new(seed_hash, init_threads)?;
        Ok(Self::with_state(state))
    }

    /// Compute a RandomX hash. Returns 32-byte hash.
    pub fn hash(
        &mut self,
        input: &[u8],
    ) -> Result<[u8; RANDOMX_HASH_SIZE], Box<dyn std::error::Error>> {
        let mut output = [0u8; RANDOMX_HASH_SIZE];
        unsafe {
            ffi::randomx_calculate_hash(
                self.vm,
                input.as_ptr() as *const _,
                input.len(),
                output.as_mut_ptr() as *mut _,
            );
        }
        Ok(output)
    }
}

impl Hasher for RandomXHasher {
    fn hash(&mut self, input: &[u8]) -> Result<[u8; 32], Box<dyn std::error::Error + Send + Sync>> {
        let mut output = [0u8; RANDOMX_HASH_SIZE];
        unsafe {
            ffi::randomx_calculate_hash(
                self.vm,
                input.as_ptr() as *const _,
                input.len(),
                output.as_mut_ptr() as *mut _,
            );
        }
        Ok(output)
    }

    fn algorithm(&self) -> Algorithm {
        Algorithm::RandomX
    }
}

impl Drop for RandomXHasher {
    fn drop(&mut self) {
        unsafe {
            if !self.vm.is_null() {
                ffi::randomx_destroy_vm(self.vm);
            }
        }
    }
}

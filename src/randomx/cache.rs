use argon2::{Argon2, Algorithm, Version, Params};

use super::{RANDOMX_ARGON_MEMORY, RANDOMX_ARGON_ITERATIONS, RANDOMX_ARGON_LANES, RANDOMX_ARGON_SALT};

/// RandomX cache: 256MB of Argon2d-derived data from the seed hash.
///
/// The cache is used in light mode for on-the-fly dataset item computation,
/// and in full mode to initialize the 2GB dataset.
pub struct RandomXCache {
    /// The cache memory (256MB)
    pub memory: Vec<u8>,
}

impl RandomXCache {
    /// Initialize the cache from a 32-byte seed hash.
    ///
    /// Uses Argon2d with RandomX-specific parameters:
    /// - Memory: 262144 KB (256 MB)
    /// - Iterations: 3
    /// - Lanes: 1
    /// - Salt: "RandomX\x03"
    pub fn new(seed_hash: &[u8; 32]) -> Result<Self, Box<dyn std::error::Error>> {
        let params = Params::new(
            RANDOMX_ARGON_MEMORY,
            RANDOMX_ARGON_ITERATIONS,
            RANDOMX_ARGON_LANES,
            None,
        ).map_err(|e| format!("Argon2 params error: {}", e))?;

        let argon2 = Argon2::new(Algorithm::Argon2d, Version::V0x13, params);

        // Argon2 output is the full memory allocation
        let memory_size = RANDOMX_ARGON_MEMORY as usize * 1024; // KB to bytes
        let mut memory = vec![0u8; memory_size];

        // Use argon2 to derive the cache memory
        log::info!("Starting Argon2d ({} MB, {} iterations)...", memory_size / (1024 * 1024), RANDOMX_ARGON_ITERATIONS);
        let t = std::time::Instant::now();
        argon2.hash_password_into(seed_hash, RANDOMX_ARGON_SALT, &mut memory)
            .map_err(|e| format!("Argon2 hash error: {}", e))?;
        log::info!("Argon2d complete in {:?}", t.elapsed());

        log::info!("RandomX cache initialized ({} MB)", memory_size / (1024 * 1024));

        Ok(Self { memory })
    }

    pub fn size(&self) -> usize {
        self.memory.len()
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.memory.as_ptr()
    }
}

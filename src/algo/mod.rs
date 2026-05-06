//! Mining algorithm abstraction.
//!
//! This module defines the supported hashing algorithm, a unified [`Hasher`]
//! trait that the algorithm implements, and an [`AlgoState`] type that holds
//! per-algorithm shared state (e.g. RandomX cache+dataset).
//!
//! The published build targets Monero/RandomX only.

use std::error::Error;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::randomx::hash::{RandomXHasher, RandomXState};

/// A mining algorithm variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    #[serde(alias = "rx/0", alias = "randomx")]
    RandomX,
}

impl Default for Algorithm {
    fn default() -> Self {
        Algorithm::RandomX
    }
}

impl Algorithm {
    /// Canonical pool-protocol algo name (matches what stratum pools advertise).
    pub fn name(&self) -> &'static str {
        match self {
            Algorithm::RandomX => "rx/0",
        }
    }

    /// Byte offset of the 4-byte nonce inside a mining-blob of the given length.
    ///
    /// For RandomX/CryptoNote this is fixed at byte 39.
    pub fn nonce_offset(&self, _blob_len: usize) -> usize {
        match self {
            Algorithm::RandomX => 39,
        }
    }

    /// Conventional default TCP port for this algorithm's pools.
    pub fn default_port(&self) -> u16 {
        match self {
            Algorithm::RandomX => 3333,
        }
    }

    /// Hash output size in bytes.
    pub fn hash_size(&self) -> usize {
        32
    }
}

/// Unified per-thread hasher trait.
///
/// Each worker owns one `Box<dyn Hasher>`. Implementations may hold per-VM
/// scratch buffers but must not be `Sync` — shared state lives in [`AlgoState`].
pub trait Hasher: Send {
    fn hash(&mut self, input: &[u8]) -> Result<[u8; 32], Box<dyn Error + Send + Sync>>;
    fn algorithm(&self) -> Algorithm;
}

/// Per-algorithm shared state.
///
/// RandomX requires a multi-GB cache+dataset that is expensive to build and
/// should be shared across all worker threads via `Arc`.
pub enum AlgoState {
    RandomX(Arc<RandomXState>),
}

impl AlgoState {
    /// Which algorithm does this state belong to?
    pub fn algorithm(&self) -> Algorithm {
        match self {
            AlgoState::RandomX(_) => Algorithm::RandomX,
        }
    }

    /// Build the shared state for the given algorithm.
    ///
    /// `seed_hash` is consulted by RandomX to initialize the cache+dataset.
    pub fn for_algo(
        algo: Algorithm,
        seed_hash: &[u8; 32],
        init_threads: usize,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        match algo {
            Algorithm::RandomX => {
                let state = RandomXState::new(seed_hash, init_threads)
                    .map_err(|e| -> Box<dyn Error + Send + Sync> { e.to_string().into() })?;
                Ok(AlgoState::RandomX(state))
            }
        }
    }

    /// Build a fresh per-thread hasher that references this shared state.
    pub fn create_hasher(&self) -> Result<Box<dyn Hasher>, Box<dyn Error + Send + Sync>> {
        match self {
            AlgoState::RandomX(state) => Ok(Box::new(RandomXHasher::with_state(state.clone()))),
        }
    }
}

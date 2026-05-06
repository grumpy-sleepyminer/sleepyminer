use std::sync::atomic::{AtomicU32, Ordering};

/// Thread-safe nonce distributor.
/// Each worker calls `next()` to get a unique nonce.
pub struct NonceCounter {
    counter: AtomicU32,
}

impl NonceCounter {
    pub fn new(start: u32) -> Self {
        Self {
            counter: AtomicU32::new(start),
        }
    }

    pub fn next(&self) -> u32 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }

    pub fn reset(&self, value: u32) {
        self.counter.store(value, Ordering::Relaxed);
    }
}

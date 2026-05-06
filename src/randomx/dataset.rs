use super::cache::RandomXCache;
use super::RANDOMX_DATASET_ITEM_COUNT;

const CACHE_LINE_SIZE: usize = 64;
const SUPERSCALAR_MUL_0: u64 = 6364136223846793005;
const SUPERSCALAR_ADD_1: u64 = 9298411001130361340;
const SUPERSCALAR_ADD_2: u64 = 12065312585734608966;
const SUPERSCALAR_ADD_3: u64 = 9306329213124626780;
const SUPERSCALAR_ADD_4: u64 = 5281919268842080866;
const SUPERSCALAR_ADD_5: u64 = 10536153434571861004;
const SUPERSCALAR_ADD_6: u64 = 3398623926847679864;
const SUPERSCALAR_ADD_7: u64 = 9549104520008361294;
const SUPERSCALAR_ADDS: [u64; 7] = [
    SUPERSCALAR_ADD_1, SUPERSCALAR_ADD_2, SUPERSCALAR_ADD_3,
    SUPERSCALAR_ADD_4, SUPERSCALAR_ADD_5, SUPERSCALAR_ADD_6,
    SUPERSCALAR_ADD_7,
];

/// Full RandomX dataset (2GB).
pub struct RandomXDataset {
    pub memory: Vec<u8>,
}

impl RandomXDataset {
    /// Initialize the full 2GB dataset from the cache using pure Rust.
    pub fn new(cache: &RandomXCache, num_threads: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let total_items = RANDOMX_DATASET_ITEM_COUNT;
        let dataset_size = total_items * CACHE_LINE_SIZE;
        let mut memory = vec![0u8; dataset_size];

        log::info!("Initializing RandomX dataset ({} MB) with {} threads...",
            dataset_size / (1024 * 1024), num_threads);

        let t = std::time::Instant::now();
        let cache_ptr = cache.as_ptr() as usize;
        let cache_size = cache.size();

        // Divide items across threads
        let items_per_thread = (total_items + num_threads - 1) / num_threads;

        // Use raw pointer for parallel mutable access to non-overlapping regions
        let mem_ptr = memory.as_mut_ptr() as usize;
        let mem_len = memory.len();

        std::thread::scope(|s| {
            for tid in 0..num_threads {
                let start = tid * items_per_thread;
                let end = ((tid + 1) * items_per_thread).min(total_items);
                if start >= end {
                    continue;
                }

                let cp = cache_ptr;
                let cs = cache_size;
                let mp = mem_ptr;

                s.spawn(move || {
                    for item in start..end {
                        let byte_offset = item * CACHE_LINE_SIZE;
                        let out = unsafe {
                            std::slice::from_raw_parts_mut(
                                (mp + byte_offset) as *mut u8,
                                CACHE_LINE_SIZE,
                            )
                        };
                        calc_dataset_item(cp as *const u8, cs, item as u64, out);
                    }
                });
            }
        });

        log::info!("Dataset initialization complete in {:.1}s", t.elapsed().as_secs_f64());

        Ok(Self { memory })
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.memory.as_ptr()
    }

    pub fn size(&self) -> usize {
        self.memory.len()
    }
}

/// Calculate a single 64-byte dataset item from the cache.
fn calc_dataset_item(cache: *const u8, cache_size: usize, item_number: u64, output: &mut [u8]) {
    let mut rl = [0u64; 8];

    // Initialize registers
    rl[0] = (item_number.wrapping_add(1)).wrapping_mul(SUPERSCALAR_MUL_0);
    for i in 1..8 {
        rl[i] = rl[0] ^ SUPERSCALAR_ADDS[i - 1];
    }

    let cache_mask = ((cache_size / CACHE_LINE_SIZE) - 1) as u64;

    // 8 cache accesses with mixing
    for _ in 0..8 {
        let cache_index = (rl[0] & cache_mask) as usize;
        let cache_offset = cache_index * CACHE_LINE_SIZE;

        // Load 64 bytes from cache
        let mut cache_line = [0u64; 8];
        unsafe {
            let src = cache.add(cache_offset) as *const u64;
            for j in 0..8 {
                cache_line[j] = std::ptr::read_unaligned(src.add(j));
            }
        }

        // XOR with cache line and mix
        for j in 0..8 {
            rl[j] ^= cache_line[j];
        }

        // SuperScalar-like mixing between registers
        let tmp = rl[0];
        rl[0] = rl[0].wrapping_mul(SUPERSCALAR_MUL_0).wrapping_add(rl[1]);
        rl[1] = rl[1].rotate_right(13) ^ rl[2];
        rl[2] = rl[2].wrapping_add(rl[3]);
        rl[3] = rl[3].rotate_right(17) ^ rl[4];
        rl[4] = rl[4].wrapping_mul(SUPERSCALAR_MUL_0);
        rl[5] = rl[5].rotate_right(23) ^ rl[6];
        rl[6] = rl[6].wrapping_add(rl[7]);
        rl[7] = rl[7].wrapping_add(tmp);
    }

    // Store result
    for j in 0..8 {
        output[j * 8..(j + 1) * 8].copy_from_slice(&rl[j].to_le_bytes());
    }
}

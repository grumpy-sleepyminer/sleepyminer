use super::cache::RandomXCache;
use super::dataset::RandomXDataset;
use super::{RANDOMX_SCRATCHPAD_L3, RANDOMX_REG_FILE_SIZE, RANDOMX_PROGRAM_ITERATIONS};
use super::ffi;

/// Aligned buffer for scratchpad and register file.
pub struct AlignedBuffer {
    data: Vec<u8>,
    offset: usize,
    usable_len: usize,
}

impl AlignedBuffer {
    pub fn new(size: usize, alignment: usize) -> Self {
        let data = vec![0u8; size + alignment];
        let ptr = data.as_ptr() as usize;
        let offset = (alignment - (ptr % alignment)) % alignment;
        Self { data, offset, usable_len: size }
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        unsafe { self.data.as_mut_ptr().add(self.offset) }
    }

    pub fn as_ptr(&self) -> *const u8 {
        unsafe { self.data.as_ptr().add(self.offset) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.data.as_mut_ptr().add(self.offset), self.usable_len) }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data.as_ptr().add(self.offset), self.usable_len) }
    }
}

/// RandomX virtual machine state.
///
/// Each mining thread should have its own VM instance.
pub struct RandomXVm {
    /// Register file (256+ bytes, 16-byte aligned)
    pub reg_file: AlignedBuffer,
    /// Memory buffer for assembly: [mx_ma: u64, dataset_ptr: u64]
    pub mem_buffer: AlignedBuffer,
    /// Reference to the cache (for light mode)
    pub cache_ptr: *const u8,
    /// Whether we have a full dataset
    pub has_dataset: bool,
}

impl RandomXVm {
    pub fn new_light(cache: &RandomXCache) -> Self {
        let reg_file = AlignedBuffer::new(RANDOMX_REG_FILE_SIZE + 256, 16);
        let mut mem_buffer = AlignedBuffer::new(16, 16);

        // In light mode, mem_buffer[8..16] points to cache
        let cache_ptr_val = cache.as_ptr() as u64;
        let buf = mem_buffer.as_mut_slice();
        buf[0..8].copy_from_slice(&0u64.to_ne_bytes());
        buf[8..16].copy_from_slice(&cache_ptr_val.to_ne_bytes());

        Self {
            reg_file,
            mem_buffer,
            cache_ptr: cache.as_ptr(),
            has_dataset: false,
        }
    }

    pub fn new_full(cache: &RandomXCache, dataset: &RandomXDataset) -> Self {
        let reg_file = AlignedBuffer::new(RANDOMX_REG_FILE_SIZE + 256, 16);
        let mut mem_buffer = AlignedBuffer::new(16, 16);

        let dataset_ptr = dataset.as_ptr() as u64;
        let buf = mem_buffer.as_mut_slice();
        buf[0..8].copy_from_slice(&0u64.to_ne_bytes());
        buf[8..16].copy_from_slice(&dataset_ptr.to_ne_bytes());

        Self {
            reg_file,
            mem_buffer,
            cache_ptr: cache.as_ptr(),
            has_dataset: true,
        }
    }

    /// Execute one RandomX program on the current state.
    pub fn execute_program(&mut self) {
        unsafe {
            ffi::randomx_program_aarch64(
                self.reg_file.as_mut_ptr(),
                self.mem_buffer.as_mut_ptr(),
                std::ptr::null_mut(), // scratchpad now managed externally
                RANDOMX_PROGRAM_ITERATIONS,
            );
        }
    }
}

// FFI bindings to the standalone RandomX C library (tevador/RandomX).
// Built as librandomx.a and linked at build time.

use std::os::raw::c_void;

pub type RandomxFlags = u32;
pub type RandomxCache = c_void;
pub type RandomxDataset = c_void;
pub type RandomxVm = c_void;

// Flag constants
pub const RANDOMX_FLAG_DEFAULT: RandomxFlags = 0;
pub const RANDOMX_FLAG_LARGE_PAGES: RandomxFlags = 1;
pub const RANDOMX_FLAG_HARD_AES: RandomxFlags = 2;
pub const RANDOMX_FLAG_FULL_MEM: RandomxFlags = 4;
pub const RANDOMX_FLAG_JIT: RandomxFlags = 8;

extern "C" {
    pub fn randomx_get_flags() -> RandomxFlags;

    pub fn randomx_alloc_cache(flags: RandomxFlags) -> *mut RandomxCache;
    pub fn randomx_init_cache(cache: *mut RandomxCache, key: *const c_void, key_size: usize);
    pub fn randomx_release_cache(cache: *mut RandomxCache);

    pub fn randomx_alloc_dataset(flags: RandomxFlags) -> *mut RandomxDataset;
    pub fn randomx_dataset_item_count() -> u64;
    pub fn randomx_init_dataset(
        dataset: *mut RandomxDataset,
        cache: *mut RandomxCache,
        start_item: u64,
        item_count: u64,
    );
    pub fn randomx_release_dataset(dataset: *mut RandomxDataset);

    pub fn randomx_create_vm(
        flags: RandomxFlags,
        cache: *mut RandomxCache,
        dataset: *mut RandomxDataset,
    ) -> *mut RandomxVm;
    pub fn randomx_vm_set_cache(machine: *mut RandomxVm, cache: *mut RandomxCache);
    pub fn randomx_vm_set_dataset(machine: *mut RandomxVm, dataset: *mut RandomxDataset);
    pub fn randomx_destroy_vm(machine: *mut RandomxVm);

    pub fn randomx_calculate_hash(
        machine: *mut RandomxVm,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
    );

    pub fn randomx_calculate_hash_first(
        machine: *mut RandomxVm,
        input: *const c_void,
        input_size: usize,
    );
    pub fn randomx_calculate_hash_next(
        machine: *mut RandomxVm,
        next_input: *const c_void,
        next_input_size: usize,
        output: *mut c_void,
    );
    pub fn randomx_calculate_hash_last(machine: *mut RandomxVm, output: *mut c_void);
}

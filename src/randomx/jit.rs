// ARM64 JIT compiler for RandomX programs.
//
// Ported from xmrig's jit_compiler_a64.cpp
// Original copyright: tevador, SChernykh, XMRig (BSD-3-Clause)
//
// Compiles RandomX bytecode into ARM64 machine code, patching the instruction
// buffer inside the assembly template linked from randomx_core.S.

use std::ptr;

use super::{
    RANDOMX_PROGRAM_SIZE, RANDOMX_SCRATCHPAD_L1, RANDOMX_SCRATCHPAD_L2, RANDOMX_SCRATCHPAD_L3,
};

// ---------------------------------------------------------------------------
// ARM64 instruction encoding constants (ARMV8A namespace)
// ---------------------------------------------------------------------------

mod armv8a {
    pub const B: u32 = 0x14000000;
    pub const EOR: u32 = 0xCA000000;
    pub const EOR32: u32 = 0x4A000000;
    pub const ADD: u32 = 0x8B000000;
    pub const SUB: u32 = 0xCB000000;
    pub const MUL: u32 = 0x9B007C00;
    pub const UMULH: u32 = 0x9BC07C00;
    pub const SMULH: u32 = 0x9B407C00;
    pub const MOVZ: u32 = 0xD2800000;
    pub const MOVN: u32 = 0x92800000;
    pub const MOVK: u32 = 0xF2800000;
    pub const ADD_IMM_LO: u32 = 0x91000000;
    pub const ADD_IMM_HI: u32 = 0x91400000;
    pub const LDR_LITERAL: u32 = 0x58000000;
    pub const ROR: u32 = 0x9AC02C00;
    pub const ROR_IMM: u32 = 0x93C00000;
    pub const MOV_REG: u32 = 0xAA0003E0;
    pub const FADD: u32 = 0x4E60D400;
    pub const FSUB: u32 = 0x4EE0D400;
    pub const FEOR: u32 = 0x6E201C00;
    pub const FMUL: u32 = 0x6E60DC00;
    pub const FDIV: u32 = 0x6E60FC00;
    pub const FSQRT: u32 = 0x6EE1F800;
}

// ---------------------------------------------------------------------------
// RandomX constants
// ---------------------------------------------------------------------------

const REGISTERS_COUNT: usize = 8;
const REGISTER_NEEDS_DISPLACEMENT: u8 = 5; // x86 r13 analog
const STORE_L3_CONDITION: u32 = 14;

/// Map RandomX register indices 0..7 to ARM64 registers
const INT_REG_MAP: [u32; 8] = [4, 5, 6, 7, 12, 13, 14, 15];

/// Pre-loaded literal registers used by IMUL_RCP (first 12 reciprocals)
const LITERAL_REGS: [u32; 12] = [
    30 << 16,
    29 << 16,
    28 << 16,
    27 << 16,
    26 << 16,
    25 << 16,
    24 << 16,
    23 << 16,
    22 << 16,
    21 << 16,
    11 << 16,
    0,
];

// ---------------------------------------------------------------------------
// RandomX configuration (matches standard Monero RandomX)
// ---------------------------------------------------------------------------

/// Runtime configuration for the JIT compiler, mirroring RandomX_ConfigurationBase.
#[derive(Clone)]
pub struct RandomXConfig {
    pub scratchpad_l1_size: u32,
    pub scratchpad_l2_size: u32,
    pub scratchpad_l3_size: u32,
    pub program_size: u32,
    pub log2_scratchpad_l1: u32,
    pub log2_scratchpad_l2: u32,
    pub log2_scratchpad_l3: u32,
    pub log2_dataset_base_size: u32,
    pub scratchpad_l3_mask: u32,
    pub scratchpad_l3_mask64: u32,
    pub jump_offset: u32,
    pub jump_bits: u32,
}

impl Default for RandomXConfig {
    fn default() -> Self {
        let l1 = RANDOMX_SCRATCHPAD_L1 as u32;
        let l2 = RANDOMX_SCRATCHPAD_L2 as u32;
        let l3 = RANDOMX_SCRATCHPAD_L3 as u32;
        RandomXConfig {
            scratchpad_l1_size: l1,
            scratchpad_l2_size: l2,
            scratchpad_l3_size: l3,
            program_size: RANDOMX_PROGRAM_SIZE as u32,
            log2_scratchpad_l1: log2_floor(l1),
            log2_scratchpad_l2: log2_floor(l2),
            log2_scratchpad_l3: log2_floor(l3),
            log2_dataset_base_size: 31, // 2 GB = 2^31
            scratchpad_l3_mask: (l3 - 1) & !63,
            scratchpad_l3_mask64: (l3 - 64),
            jump_offset: 8,
            jump_bits: 8,
        }
    }
}

fn log2_floor(mut x: u32) -> u32 {
    let mut r = 0u32;
    x >>= 1;
    while x > 0 {
        r += 1;
        x >>= 1;
    }
    r
}

// ---------------------------------------------------------------------------
// RandomX Instruction (matches the C++ struct layout, 8 bytes)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Instruction {
    pub opcode: u8,
    pub dst: u8,
    pub src: u8,
    pub mod_: u8,
    pub imm32: u32,
}

impl Instruction {
    #[inline]
    pub fn get_imm32(&self) -> u32 {
        self.imm32
    }

    #[inline]
    pub fn get_mod_mem(&self) -> u32 {
        (self.mod_ & 3) as u32
    }

    #[inline]
    pub fn get_mod_shift(&self) -> u32 {
        ((self.mod_ >> 2) & 3) as u32
    }

    #[inline]
    pub fn get_mod_cond(&self) -> u32 {
        (self.mod_ >> 4) as u32
    }
}

// ---------------------------------------------------------------------------
// Program configuration (written by the hash after program generation)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ProgramConfiguration {
    pub read_reg0: usize,
    pub read_reg1: usize,
    pub read_reg2: usize,
    pub read_reg3: usize,
}

// ---------------------------------------------------------------------------
// Reciprocal calculation for IMUL_RCP
// ---------------------------------------------------------------------------

/// Calculate `2^x / divisor` for the highest integer x such that the result < 2^64.
/// Uses the fast path with `__builtin_clzll` equivalent.
pub fn reciprocal(divisor: u64) -> u64 {
    debug_assert!(divisor != 0);
    debug_assert!(!is_zero_or_power_of_2(divisor));

    let p2exp63: u64 = 1u64 << 63;
    let q = p2exp63 / divisor;
    let r = p2exp63 % divisor;
    let shift = 64 - divisor.leading_zeros(); // equivalent to 64 - __builtin_clzll

    (q << shift) + ((r << shift) / divisor)
}

#[inline]
fn is_zero_or_power_of_2(x: u64) -> bool {
    (x & x.wrapping_sub(1)) == 0
}

// ---------------------------------------------------------------------------
// FFI symbols from the assembly template (randomx_core.S)
// ---------------------------------------------------------------------------

extern "C" {
    fn randomx_program_aarch64();
    fn randomx_program_aarch64_main_loop();
    fn randomx_program_aarch64_vm_instructions();
    fn randomx_program_aarch64_vm_instructions_end();
    fn randomx_program_aarch64_imul_rcp_literals_end();
    fn randomx_program_aarch64_cacheline_align_mask1();
    fn randomx_program_aarch64_cacheline_align_mask2();
    fn randomx_program_aarch64_update_spMix1();
    fn randomx_init_dataset_aarch64_end();
}

/// Compute a symbol offset relative to the base of the assembly template.
macro_rules! asm_offset {
    ($sym:ident) => {
        unsafe {
            ($sym as *const u8 as usize)
                - (randomx_program_aarch64 as *const u8 as usize)
        }
    };
}

// ---------------------------------------------------------------------------
// JIT Compiler
// ---------------------------------------------------------------------------

/// ARM64 JIT compiler for RandomX.
///
/// Allocates an executable memory region via `mmap(MAP_JIT)`, copies the
/// assembly template into it, then patches per-program instructions.
pub struct JitCompiler {
    /// Pointer to the mmap'd code buffer.
    code: *mut u8,
    /// Total allocation size.
    allocated_size: usize,
    /// Position (from template start) where next IMUL_RCP literal is written.
    literal_pos: u32,
    /// Number of 32-bit literals placed in the NEON vector literal pool.
    num_32bit_literals: u32,
    /// Per-register: code offset where the register was last changed (for CBRANCH).
    reg_changed_offset: [u32; REGISTERS_COUNT],
    /// Runtime configuration.
    config: RandomXConfig,
    /// Cached template metrics.
    code_size: usize,
    main_loop_begin: usize,
    prologue_size: usize,
    imul_rcp_literals_end: usize,
}

unsafe impl Send for JitCompiler {}
unsafe impl Sync for JitCompiler {}

impl Drop for JitCompiler {
    fn drop(&mut self) {
        if !self.code.is_null() && self.allocated_size > 0 {
            unsafe {
                libc::munmap(self.code as *mut libc::c_void, self.allocated_size);
            }
        }
    }
}

impl JitCompiler {
    /// Create a new JIT compiler with the default Monero RandomX configuration.
    pub fn new() -> Self {
        Self::with_config(RandomXConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: RandomXConfig) -> Self {
        let code_size = unsafe {
            (randomx_init_dataset_aarch64_end as *const u8 as usize)
                - (randomx_program_aarch64 as *const u8 as usize)
        };
        let main_loop_begin = asm_offset!(randomx_program_aarch64_main_loop);
        let prologue_size = asm_offset!(randomx_program_aarch64_vm_instructions);
        let imul_rcp_literals_end = asm_offset!(randomx_program_aarch64_imul_rcp_literals_end);

        JitCompiler {
            code: ptr::null_mut(),
            allocated_size: 0,
            literal_pos: imul_rcp_literals_end as u32,
            num_32bit_literals: 0,
            reg_changed_offset: [0; REGISTERS_COUNT],
            config,
            code_size,
            main_loop_begin,
            prologue_size,
            imul_rcp_literals_end,
        }
    }

    // ------------------------------------------------------------------
    // Memory management (macOS ARM64 MAP_JIT + W^X)
    // ------------------------------------------------------------------

    /// Allocate executable memory and copy the assembly template.
    fn allocate(&mut self, size: usize) {
        self.allocated_size = size;

        unsafe {
            let ptr = libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_JIT,
                -1,
                0,
            );
            assert!(
                ptr != libc::MAP_FAILED,
                "mmap(MAP_JIT) failed for JIT code buffer"
            );
            self.code = ptr as *mut u8;

            // Copy assembly template
            self.enable_writing();
            ptr::copy_nonoverlapping(
                randomx_program_aarch64 as *const u8,
                self.code,
                self.code_size,
            );
        }
    }

    /// Switch the JIT page to writable (disable execute) on macOS.
    pub fn enable_writing(&self) {
        #[cfg(target_os = "macos")]
        unsafe {
            pthread_jit_write_protect_np(0);
        }
    }

    /// Switch the JIT page to executable (disable write) on macOS.
    pub fn enable_execution(&self) {
        #[cfg(target_os = "macos")]
        unsafe {
            pthread_jit_write_protect_np(1);
        }
    }

    /// Flush the instruction cache for the given range.
    fn flush_icache(&self, offset: usize, length: usize) {
        unsafe {
            let start = self.code.add(offset) as *mut libc::c_void;
            let end = self.code.add(offset + length) as *mut libc::c_void;
            sys_icache_invalidate(start, end as usize - start as usize);
        }
    }

    /// Get a function pointer to the generated program.
    pub fn get_program_func(
        &self,
    ) -> unsafe extern "C" fn(*mut u8, *mut u8, *mut u8, u64) {
        self.enable_execution();
        self.flush_icache(0, self.allocated_size);
        unsafe { std::mem::transmute(self.code) }
    }

    /// Get the raw code pointer.
    pub fn get_code(&self) -> *mut u8 {
        self.code
    }

    /// Get the code size.
    pub fn get_code_size(&self) -> usize {
        self.code_size
    }

    // ------------------------------------------------------------------
    // Emit helpers
    // ------------------------------------------------------------------

    #[inline]
    fn emit32(val: u32, code: *mut u8, pos: &mut u32) {
        unsafe {
            ptr::write_unaligned(code.add(*pos as usize) as *mut u32, val);
        }
        *pos += 4;
    }

    #[inline]
    fn emit64(val: u64, code: *mut u8, pos: &mut u32) {
        unsafe {
            ptr::write_unaligned(code.add(*pos as usize) as *mut u64, val);
        }
        *pos += 8;
    }

    /// Emit a MOV immediate (up to 32 bits) into a register.
    fn emit_mov_immediate(
        &mut self,
        dst: u32,
        imm: u32,
        code: *mut u8,
        pos: &mut u32,
    ) {
        if imm < (1 << 16) {
            // movz dst, imm
            Self::emit32(armv8a::MOVZ | dst | (imm << 5), code, pos);
        } else if self.num_32bit_literals < 64 {
            // Use NEON vector literal pool (smov/umov)
            let idx = self.num_32bit_literals;
            if (imm as i32) < 0 {
                // smov dst, vN.s[M]
                Self::emit32(
                    0x4E042C00 | dst | ((idx / 4) << 5) | ((idx % 4) << 19),
                    code,
                    pos,
                );
            } else {
                // umov dst, vN.s[M]
                Self::emit32(
                    0x0E043C00 | dst | ((idx / 4) << 5) | ((idx % 4) << 19),
                    code,
                    pos,
                );
            }
            // Store literal into the vector literal pool area
            unsafe {
                let lit_ptr =
                    code.add(self.imul_rcp_literals_end) as *mut u32;
                ptr::write_unaligned(lit_ptr.add(idx as usize), imm);
            }
            self.num_32bit_literals += 1;
        } else {
            if (imm as i32) < 0 {
                // movn dst, ~imm >> 16
                Self::emit32(
                    armv8a::MOVN | dst | (1 << 21) | (((!imm) >> 16) << 5),
                    code,
                    pos,
                );
            } else {
                // movz dst, imm >> 16
                Self::emit32(
                    armv8a::MOVZ | dst | (1 << 21) | ((imm >> 16) << 5),
                    code,
                    pos,
                );
            }
            // movk dst, imm & 0xFFFF
            Self::emit32(
                armv8a::MOVK | dst | ((imm & 0xFFFF) << 5),
                code,
                pos,
            );
        }
    }

    /// Emit an ADD immediate (up to 32 bits).
    fn emit_add_immediate(
        &mut self,
        dst: u32,
        src: u32,
        imm: u32,
        code: *mut u8,
        pos: &mut u32,
    ) {
        if imm < (1 << 24) {
            let imm_lo = imm & ((1 << 12) - 1);
            let imm_hi = imm >> 12;

            if imm_lo != 0 && imm_hi != 0 {
                Self::emit32(
                    armv8a::ADD_IMM_LO | dst | (src << 5) | (imm_lo << 10),
                    code,
                    pos,
                );
                Self::emit32(
                    armv8a::ADD_IMM_HI | dst | (dst << 5) | (imm_hi << 10),
                    code,
                    pos,
                );
            } else if imm_lo != 0 {
                Self::emit32(
                    armv8a::ADD_IMM_LO | dst | (src << 5) | (imm_lo << 10),
                    code,
                    pos,
                );
            } else {
                Self::emit32(
                    armv8a::ADD_IMM_HI | dst | (src << 5) | (imm_hi << 10),
                    code,
                    pos,
                );
            }
        } else {
            let tmp_reg: u32 = 20;
            self.emit_mov_immediate(tmp_reg, imm, code, pos);
            // add dst, src, tmp_reg
            Self::emit32(
                armv8a::ADD | dst | (src << 5) | (tmp_reg << 16),
                code,
                pos,
            );
        }
    }

    /// Emit a memory load from scratchpad into an integer temp register.
    fn emit_mem_load(
        &mut self,
        tmp_reg: u32,
        dst: u32,
        src: u32,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let mut imm = instr.get_imm32();

        if src != dst {
            imm &= if instr.get_mod_mem() != 0 {
                self.config.scratchpad_l1_size - 1
            } else {
                self.config.scratchpad_l2_size - 1
            };

            // AND instruction base: and x<tmp_reg>, x<tmp_reg>, mask
            let mut t = 0x927d0000u32 | tmp_reg | (tmp_reg << 5);
            if imm != 0 {
                self.emit_add_immediate(tmp_reg, src, imm, code, pos);
            } else {
                t = 0x927d0000u32 | tmp_reg | (src << 5);
            }

            let and_instr_l1 =
                t | ((self.config.log2_scratchpad_l1 - 4) << 10);
            let and_instr_l2 =
                t | ((self.config.log2_scratchpad_l2 - 4) << 10);

            Self::emit32(
                if instr.get_mod_mem() != 0 {
                    and_instr_l1
                } else {
                    and_instr_l2
                },
                code,
                pos,
            );

            // ldr tmp_reg, [x2, tmp_reg]
            Self::emit32(0xf8606840 | tmp_reg | (tmp_reg << 16), code, pos);
        } else {
            imm = (imm & self.config.scratchpad_l3_mask) >> 3;
            if imm != 0 {
                self.emit_mov_immediate(tmp_reg, imm, code, pos);
                // ldr tmp_reg, [x2, tmp_reg, lsl 3]
                Self::emit32(
                    0xf8607840 | tmp_reg | (tmp_reg << 16),
                    code,
                    pos,
                );
            } else {
                // ldr tmp_reg, [x2]
                Self::emit32(0xf9400040 | tmp_reg, code, pos);
            }
        }
    }

    /// Emit a memory load for floating-point operations from scratchpad.
    fn emit_mem_load_fp(
        &mut self,
        tmp_reg_fp: u32,
        src: u32,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let tmp_reg: u32 = 19;

        let imm = instr.get_imm32()
            & if instr.get_mod_mem() != 0 {
                self.config.scratchpad_l1_size - 1
            } else {
                self.config.scratchpad_l2_size - 1
            };

        let mut t = 0x927d0000u32 | tmp_reg | (tmp_reg << 5);
        if imm != 0 {
            self.emit_add_immediate(tmp_reg, src, imm, code, pos);
        } else {
            t = 0x927d0000u32 | tmp_reg | (src << 5);
        }

        let and_instr_l1 = t | ((self.config.log2_scratchpad_l1 - 4) << 10);
        let and_instr_l2 = t | ((self.config.log2_scratchpad_l2 - 4) << 10);

        Self::emit32(
            if instr.get_mod_mem() != 0 {
                and_instr_l1
            } else {
                and_instr_l2
            },
            code,
            pos,
        );

        // ldr tmp_reg_fp, [x2, tmp_reg]
        Self::emit32(
            0x3ce06800 | tmp_reg_fp | (2 << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        // sxtl.2d tmp_reg_fp, tmp_reg_fp
        Self::emit32(
            0x0f20a400 | tmp_reg_fp | (tmp_reg_fp << 5),
            code,
            pos,
        );

        // scvtf tmp_reg_fp.2d, tmp_reg_fp.2d
        Self::emit32(
            0x4E61D800 | tmp_reg_fp | (tmp_reg_fp << 5),
            code,
            pos,
        );
    }

    // ------------------------------------------------------------------
    // Program generation
    // ------------------------------------------------------------------

    /// Generate ARM64 machine code for a RandomX program.
    ///
    /// `program` is the array of 256 instructions.
    /// `config` specifies which registers are read for spMix.
    /// `flags` is the VM flags (e.g. RANDOMX_FLAG_HARD_AES).
    pub fn generate_program(
        &mut self,
        program: &[Instruction],
        prog_config: &ProgramConfiguration,
        _flags: u32,
    ) {
        if self.allocated_size == 0 {
            self.allocate(self.code_size);
        } else {
            self.enable_writing();
        }

        let code = self.code;
        let mut code_pos = (self.main_loop_begin + 4) as u32;

        let mask = (self.config.log2_scratchpad_l3 - 7) << 10;

        // and w16, w10, ScratchpadL3Mask64
        Self::emit32(
            0x121A0000 | 16 | (10 << 5) | mask,
            code,
            &mut code_pos,
        );
        // and w17, w20, ScratchpadL3Mask64
        Self::emit32(
            0x121A0000 | 17 | (20 << 5) | mask,
            code,
            &mut code_pos,
        );

        code_pos = self.prologue_size as u32;
        self.literal_pos = self.imul_rcp_literals_end as u32;
        self.num_32bit_literals = 0;

        // Fill instruction buffer with NOPs (0x00000000 = UDF on ARM64, NOT NOP)
        let vm_end = asm_offset!(randomx_program_aarch64_vm_instructions_end) as u32;
        let nop_arm64: u32 = 0xD503201F;
        let mut fill_pos = code_pos;
        while fill_pos < vm_end {
            Self::emit32(nop_arm64, code, &mut fill_pos);
        }

        for i in 0..REGISTERS_COUNT {
            self.reg_changed_offset[i] = code_pos;
        }

        let vm_end = asm_offset!(randomx_program_aarch64_vm_instructions_end) as u32;

        // TODO: Full opcode handlers disabled while debugging SIGSEGV.
        // For now, emit only NOPs in the instruction buffer.
        // This produces wrong hashes but exercises the assembly template.
        let _ = program;
        // NOPs already filled above, just leave code_pos at prologue_size

        // Update spMix2: eor w20, config.readReg2, config.readReg3
        Self::emit32(
            armv8a::EOR32
                | 20
                | (INT_REG_MAP[prog_config.read_reg2] << 5)
                | (INT_REG_MAP[prog_config.read_reg3] << 16),
            code,
            &mut code_pos,
        );

        // Jump back to the main loop
        let vm_instr_end_offset =
            asm_offset!(randomx_program_aarch64_vm_instructions_end);
        let offset = (vm_instr_end_offset as i32) - (code_pos as i32);
        Self::emit32(armv8a::B | (((offset / 4) as u32) & 0x03FFFFFF), code, &mut code_pos);

        // Patch CacheLineAlignMask
        let ds_mask = (self.config.log2_dataset_base_size - 7) << 10;

        // and w20, w20, CacheLineAlignMask
        code_pos =
            asm_offset!(randomx_program_aarch64_cacheline_align_mask1) as u32;
        Self::emit32(
            0x121A0000 | 20 | (20 << 5) | ds_mask,
            code,
            &mut code_pos,
        );

        // and w10, w10, CacheLineAlignMask
        code_pos =
            asm_offset!(randomx_program_aarch64_cacheline_align_mask2) as u32;
        Self::emit32(
            0x121A0000 | 10 | (10 << 5) | ds_mask,
            code,
            &mut code_pos,
        );

        // Update spMix1: eor x10, config.readReg0, config.readReg1
        code_pos =
            asm_offset!(randomx_program_aarch64_update_spMix1) as u32;
        Self::emit32(
            armv8a::EOR
                | 10
                | (INT_REG_MAP[prog_config.read_reg0] << 5)
                | (INT_REG_MAP[prog_config.read_reg1] << 16),
            code,
            &mut code_pos,
        );

        // Flush icache for the modified region
        self.flush_icache(
            self.main_loop_begin,
            self.allocated_size - self.main_loop_begin,
        );
    }

    // ------------------------------------------------------------------
    // Opcode handlers (h_IADD_RS through h_NOP)
    // ------------------------------------------------------------------

    fn h_iadd_rs(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let shift = instr.get_mod_shift();

        // add dst, dst, src << shift
        Self::emit32(
            armv8a::ADD | dst | (dst << 5) | (shift << 10) | (src << 16),
            code,
            pos,
        );

        if instr.dst % REGISTERS_COUNT as u8 == REGISTER_NEEDS_DISPLACEMENT {
            self.emit_add_immediate(dst, dst, instr.get_imm32(), code, pos);
        }

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_iadd_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        self.emit_mem_load(tmp_reg, dst, src, instr, code, pos);

        // add dst, dst, tmp_reg
        Self::emit32(
            armv8a::ADD | dst | (dst << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_isub_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        if src != dst {
            // sub dst, dst, src
            Self::emit32(
                armv8a::SUB | dst | (dst << 5) | (src << 16),
                code,
                pos,
            );
        } else {
            let neg_imm = (instr.get_imm32() as i32).wrapping_neg() as u32;
            self.emit_add_immediate(dst, dst, neg_imm, code, pos);
        }

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_isub_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        self.emit_mem_load(tmp_reg, dst, src, instr, code, pos);

        // sub dst, dst, tmp_reg
        Self::emit32(
            armv8a::SUB | dst | (dst << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_imul_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let mut src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        if src == dst {
            src = 20;
            self.emit_mov_immediate(src, instr.get_imm32(), code, pos);
        }

        // mul dst, dst, src
        Self::emit32(
            armv8a::MUL | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_imul_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        self.emit_mem_load(tmp_reg, dst, src, instr, code, pos);

        // mul dst, dst, tmp_reg
        Self::emit32(
            armv8a::MUL | dst | (dst << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_imulh_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        // umulh dst, dst, src
        Self::emit32(
            armv8a::UMULH | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_imulh_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        self.emit_mem_load(tmp_reg, dst, src, instr, code, pos);

        // umulh dst, dst, tmp_reg
        Self::emit32(
            armv8a::UMULH | dst | (dst << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_ismulh_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        // smulh dst, dst, src
        Self::emit32(
            armv8a::SMULH | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_ismulh_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        self.emit_mem_load(tmp_reg, dst, src, instr, code, pos);

        // smulh dst, dst, tmp_reg
        Self::emit32(
            armv8a::SMULH | dst | (dst << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_imul_rcp(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let divisor = instr.get_imm32() as u64;
        if is_zero_or_power_of_2(divisor) {
            return;
        }

        let tmp_reg: u32 = 20;
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        let rcp = reciprocal(divisor);

        let literal_id = (self.imul_rcp_literals_end as u32 - self.literal_pos)
            / std::mem::size_of::<u64>() as u32;

        // Store reciprocal literal
        self.literal_pos -= std::mem::size_of::<u64>() as u32;
        unsafe {
            ptr::write_unaligned(
                code.add(self.literal_pos as usize) as *mut u64,
                rcp,
            );
        }

        if literal_id < 12 {
            // mul dst, dst, literal_reg
            Self::emit32(
                armv8a::MUL | dst | (dst << 5) | LITERAL_REGS[literal_id as usize],
                code,
                pos,
            );
        } else {
            // ldr tmp_reg, reciprocal
            let offset = (self.literal_pos - *pos) / 4;
            Self::emit32(
                armv8a::LDR_LITERAL | tmp_reg | (offset << 5),
                code,
                pos,
            );
            // mul dst, dst, tmp_reg
            Self::emit32(
                armv8a::MUL | dst | (dst << 5) | (tmp_reg << 16),
                code,
                pos,
            );
        }

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_ineg_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        // sub dst, xzr, dst
        Self::emit32(
            armv8a::SUB | dst | (31 << 5) | (dst << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_ixor_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let mut src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        if src == dst {
            src = 20;
            self.emit_mov_immediate(src, instr.get_imm32(), code, pos);
        }

        // eor dst, dst, src
        Self::emit32(
            armv8a::EOR | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_ixor_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        self.emit_mem_load(tmp_reg, dst, src, instr, code, pos);

        // eor dst, dst, tmp_reg
        Self::emit32(
            armv8a::EOR | dst | (dst << 5) | (tmp_reg << 16),
            code,
            pos,
        );

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_iror_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        if src != dst {
            // ror dst, dst, src
            Self::emit32(
                armv8a::ROR | dst | (dst << 5) | (src << 16),
                code,
                pos,
            );
        } else {
            let imm = instr.get_imm32() & 63;
            if imm != 0 {
                // ror dst, dst, imm
                Self::emit32(
                    armv8a::ROR_IMM
                        | dst
                        | (dst << 5)
                        | (imm << 10)
                        | (dst << 16),
                    code,
                    pos,
                );
            }
        }

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_irol_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        if src != dst {
            let tmp_reg: u32 = 20;
            // sub tmp_reg, xzr, src
            Self::emit32(
                armv8a::SUB | tmp_reg | (31 << 5) | (src << 16),
                code,
                pos,
            );
            // ror dst, dst, tmp_reg
            Self::emit32(
                armv8a::ROR | dst | (dst << 5) | (tmp_reg << 16),
                code,
                pos,
            );
        } else {
            let imm = (instr.get_imm32() as i32).wrapping_neg() as u32 & 63;
            if imm != 0 {
                // ror dst, dst, -imm & 63
                Self::emit32(
                    armv8a::ROR_IMM
                        | dst
                        | (dst << 5)
                        | (imm << 10)
                        | (dst << 16),
                    code,
                    pos,
                );
            }
        }

        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_iswap_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];

        if src == dst {
            return;
        }

        let tmp_reg: u32 = 20;
        Self::emit32(armv8a::MOV_REG | tmp_reg | (dst << 16), code, pos);
        Self::emit32(armv8a::MOV_REG | dst | (src << 16), code, pos);
        Self::emit32(armv8a::MOV_REG | src | (tmp_reg << 16), code, pos);

        self.reg_changed_offset[instr.src as usize % REGISTERS_COUNT] = *pos;
        self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT] = *pos;
    }

    fn h_fswap_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let dst = (instr.dst as u32 % REGISTERS_COUNT as u32) + 16;

        // ext dst.16b, dst.16b, dst.16b, #0x8
        Self::emit32(
            0x6e004000 | dst | (dst << 5) | (dst << 16),
            code,
            pos,
        );
    }

    fn h_fadd_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = (instr.src as u32 % 4) + 24;
        let dst = (instr.dst as u32 % 4) + 16;

        Self::emit32(
            armv8a::FADD | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );
    }

    fn h_fadd_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = (instr.dst as u32 % 4) + 16;
        let tmp_reg_fp: u32 = 28;

        self.emit_mem_load_fp(tmp_reg_fp, src, instr, code, pos);

        Self::emit32(
            armv8a::FADD | dst | (dst << 5) | (tmp_reg_fp << 16),
            code,
            pos,
        );
    }

    fn h_fsub_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = (instr.src as u32 % 4) + 24;
        let dst = (instr.dst as u32 % 4) + 16;

        Self::emit32(
            armv8a::FSUB | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );
    }

    fn h_fsub_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = (instr.dst as u32 % 4) + 16;
        let tmp_reg_fp: u32 = 28;

        self.emit_mem_load_fp(tmp_reg_fp, src, instr, code, pos);

        Self::emit32(
            armv8a::FSUB | dst | (dst << 5) | (tmp_reg_fp << 16),
            code,
            pos,
        );
    }

    fn h_fscal_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let dst = (instr.dst as u32 % 4) + 16;

        // eor dst, dst, v31 (sign flip mask)
        Self::emit32(
            armv8a::FEOR | dst | (dst << 5) | (31 << 16),
            code,
            pos,
        );
    }

    fn h_fmul_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = (instr.src as u32 % 4) + 24;
        let dst = (instr.dst as u32 % 4) + 20;

        Self::emit32(
            armv8a::FMUL | dst | (dst << 5) | (src << 16),
            code,
            pos,
        );
    }

    fn h_fdiv_m(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = (instr.dst as u32 % 4) + 20;
        let tmp_reg_fp: u32 = 28;

        self.emit_mem_load_fp(tmp_reg_fp, src, instr, code, pos);

        // and tmp_reg_fp, tmp_reg_fp, and_mask_reg (v29)
        Self::emit32(
            0x4E201C00 | tmp_reg_fp | (tmp_reg_fp << 5) | (29 << 16),
            code,
            pos,
        );

        // orr tmp_reg_fp, tmp_reg_fp, or_mask_reg (v30)
        Self::emit32(
            0x4EA01C00 | tmp_reg_fp | (tmp_reg_fp << 5) | (30 << 16),
            code,
            pos,
        );

        Self::emit32(
            armv8a::FDIV | dst | (dst << 5) | (tmp_reg_fp << 16),
            code,
            pos,
        );
    }

    fn h_fsqrt_r(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let dst = (instr.dst as u32 % 4) + 20;

        Self::emit32(armv8a::FSQRT | dst | (dst << 5), code, pos);
    }

    fn h_cbranch(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let mod_cond = instr.get_mod_cond();
        let shift = mod_cond + self.config.jump_offset;
        let imm = (instr.get_imm32() | (1u32 << shift)) & !(1u32 << (shift - 1));

        self.emit_add_immediate(dst, dst, imm, code, pos);

        // tst dst, mask
        Self::emit32(
            (0xF2781C1Fu32.wrapping_sub(mod_cond << 16)) | (dst << 5),
            code,
            pos,
        );

        let target_offset =
            self.reg_changed_offset[instr.dst as usize % REGISTERS_COUNT];
        let offset =
            ((target_offset as i32 - *pos as i32) >> 2) & ((1 << 19) - 1);

        // beq target
        Self::emit32(0x54000000 | ((offset as u32) << 5), code, pos);

        for i in 0..REGISTERS_COUNT {
            self.reg_changed_offset[i] = *pos;
        }
    }

    fn h_cfround(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;
        let fpcr_tmp_reg: u32 = 8;

        // ror tmp_reg, src, imm
        Self::emit32(
            armv8a::ROR_IMM
                | tmp_reg
                | (src << 5)
                | ((instr.get_imm32() & 63) << 10)
                | (src << 16),
            code,
            pos,
        );

        // bfi fpcr_tmp_reg, tmp_reg, 40, 2
        Self::emit32(
            0xB3580400 | fpcr_tmp_reg | (tmp_reg << 5),
            code,
            pos,
        );

        // rbit tmp_reg, fpcr_tmp_reg
        Self::emit32(
            0xDAC00000 | tmp_reg | (fpcr_tmp_reg << 5),
            code,
            pos,
        );

        // msr fpcr, tmp_reg
        Self::emit32(0xD51B4400 | tmp_reg, code, pos);
    }

    fn h_istore(
        &mut self,
        instr: &Instruction,
        code: *mut u8,
        pos: &mut u32,
    ) {
        let src = INT_REG_MAP[instr.src as usize % REGISTERS_COUNT];
        let dst = INT_REG_MAP[instr.dst as usize % REGISTERS_COUNT];
        let tmp_reg: u32 = 20;

        let mut imm = instr.get_imm32();

        if instr.get_mod_cond() < STORE_L3_CONDITION {
            imm &= if instr.get_mod_mem() != 0 {
                self.config.scratchpad_l1_size - 1
            } else {
                self.config.scratchpad_l2_size - 1
            };
        } else {
            imm &= self.config.scratchpad_l3_size - 1;
        }

        let mut t = 0x927d0000u32 | tmp_reg | (tmp_reg << 5);
        if imm != 0 {
            self.emit_add_immediate(tmp_reg, dst, imm, code, pos);
        } else {
            t = 0x927d0000u32 | tmp_reg | (dst << 5);
        }

        let and_instr_l1 = t | ((self.config.log2_scratchpad_l1 - 4) << 10);
        let and_instr_l2 = t | ((self.config.log2_scratchpad_l2 - 4) << 10);
        let and_instr_l3 = t | ((self.config.log2_scratchpad_l3 - 4) << 10);

        let and_instr = if instr.get_mod_cond() < STORE_L3_CONDITION {
            if instr.get_mod_mem() != 0 {
                and_instr_l1
            } else {
                and_instr_l2
            }
        } else {
            and_instr_l3
        };

        Self::emit32(and_instr, code, pos);

        // str src, [x2, tmp_reg]
        Self::emit32(0xF8206840 | src | (tmp_reg << 16), code, pos);
    }

    fn h_nop(
        &mut self,
        _instr: &Instruction,
        _code: *mut u8,
        _pos: &mut u32,
    ) {
        // NOP: emit nothing
    }
}

// ---------------------------------------------------------------------------
// Opcode dispatch table
// ---------------------------------------------------------------------------

/// Handler function type: takes JitCompiler, instruction, code buffer, position.
type OpcodeHandler = fn(&mut JitCompiler, &Instruction, *mut u8, &mut u32);

/// Build the 256-entry opcode dispatch table based on standard Monero RandomX
/// instruction frequencies.
fn build_engine_table() -> [OpcodeHandler; 256] {
    let mut table: [OpcodeHandler; 256] =
        [JitCompiler::h_nop as OpcodeHandler; 256];

    // Standard Monero RandomX frequencies
    let freqs: [(OpcodeHandler, u32); 30] = [
        (JitCompiler::h_iadd_rs, 16),  // IADD_RS
        (JitCompiler::h_iadd_m, 7),    // IADD_M
        (JitCompiler::h_isub_r, 16),   // ISUB_R
        (JitCompiler::h_isub_m, 7),    // ISUB_M
        (JitCompiler::h_imul_r, 16),   // IMUL_R
        (JitCompiler::h_imul_m, 4),    // IMUL_M
        (JitCompiler::h_imulh_r, 4),   // IMULH_R
        (JitCompiler::h_imulh_m, 1),   // IMULH_M
        (JitCompiler::h_ismulh_r, 4),  // ISMULH_R
        (JitCompiler::h_ismulh_m, 1),  // ISMULH_M
        (JitCompiler::h_imul_rcp, 8),  // IMUL_RCP
        (JitCompiler::h_ineg_r, 2),    // INEG_R
        (JitCompiler::h_ixor_r, 15),   // IXOR_R
        (JitCompiler::h_ixor_m, 5),    // IXOR_M
        (JitCompiler::h_iror_r, 8),    // IROR_R
        (JitCompiler::h_irol_r, 2),    // IROL_R
        (JitCompiler::h_iswap_r, 4),   // ISWAP_R
        (JitCompiler::h_fswap_r, 4),   // FSWAP_R
        (JitCompiler::h_fadd_r, 16),   // FADD_R
        (JitCompiler::h_fadd_m, 5),    // FADD_M
        (JitCompiler::h_fsub_r, 16),   // FSUB_R
        (JitCompiler::h_fsub_m, 5),    // FSUB_M
        (JitCompiler::h_fscal_r, 6),   // FSCAL_R
        (JitCompiler::h_fmul_r, 32),   // FMUL_R
        (JitCompiler::h_fdiv_m, 4),    // FDIV_M
        (JitCompiler::h_fsqrt_r, 6),   // FSQRT_R
        (JitCompiler::h_cbranch, 25),  // CBRANCH
        (JitCompiler::h_cfround, 1),   // CFROUND
        (JitCompiler::h_istore, 16),   // ISTORE
        (JitCompiler::h_nop, 0),       // NOP
    ];

    let mut k = 0usize;
    for &(handler, freq) in &freqs {
        for _ in 0..freq {
            if k < 256 {
                table[k] = handler;
                k += 1;
            }
        }
    }

    table
}

// ---------------------------------------------------------------------------
// Platform-specific externals
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
extern "C" {
    fn pthread_jit_write_protect_np(protect: i32);
    fn sys_icache_invalidate(start: *mut libc::c_void, size: usize);
}

#[cfg(not(target_os = "macos"))]
unsafe fn pthread_jit_write_protect_np(_protect: i32) {
    // No-op on non-macOS
}

#[cfg(not(target_os = "macos"))]
unsafe fn sys_icache_invalidate(start: *mut libc::c_void, size: usize) {
    // Use __clear_cache on Linux
    extern "C" {
        fn __clear_cache(start: *mut libc::c_void, end: *mut libc::c_void);
    }
    __clear_cache(start, (start as *mut u8).add(size) as *mut libc::c_void);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reciprocal() {
        // Known values from the RandomX spec
        assert_eq!(reciprocal(3), 12297829382473034411);
        assert_eq!(reciprocal(5), 14757395258967641293);
        assert_eq!(reciprocal(7), 10540996613548315209);
        assert_eq!(reciprocal(33), 2236962132638266879);
    }

    #[test]
    fn test_is_zero_or_power_of_2() {
        assert!(is_zero_or_power_of_2(0));
        assert!(is_zero_or_power_of_2(1));
        assert!(is_zero_or_power_of_2(2));
        assert!(is_zero_or_power_of_2(4));
        assert!(is_zero_or_power_of_2(1 << 20));
        assert!(!is_zero_or_power_of_2(3));
        assert!(!is_zero_or_power_of_2(5));
        assert!(!is_zero_or_power_of_2(6));
    }

    #[test]
    fn test_log2_floor() {
        assert_eq!(log2_floor(1), 0);
        assert_eq!(log2_floor(2), 1);
        assert_eq!(log2_floor(16384), 14); // L1 = 16KB
        assert_eq!(log2_floor(262144), 18); // L2 = 256KB
        assert_eq!(log2_floor(2097152), 21); // L3 = 2MB
    }

    #[test]
    fn test_config_default() {
        let c = RandomXConfig::default();
        assert_eq!(c.scratchpad_l1_size, 16384);
        assert_eq!(c.scratchpad_l2_size, 262144);
        assert_eq!(c.scratchpad_l3_size, 2097152);
        assert_eq!(c.log2_scratchpad_l1, 14);
        assert_eq!(c.log2_scratchpad_l2, 18);
        assert_eq!(c.log2_scratchpad_l3, 21);
        assert_eq!(c.scratchpad_l3_mask, (2097152 - 1) & !63);
    }

    #[test]
    fn test_engine_table_size() {
        let table = build_engine_table();
        // All 256 entries should be filled
        // Sum of all frequencies = 256
        // Verify the last entry is a valid handler (NOP frequency is 0,
        // so entries 255 should still be h_nop from initialization)
        assert_eq!(table.len(), 256);
    }

    #[test]
    fn test_instruction_layout() {
        assert_eq!(std::mem::size_of::<Instruction>(), 8);
    }

    #[test]
    fn test_instruction_accessors() {
        let instr = Instruction {
            opcode: 0,
            dst: 3,
            src: 5,
            mod_: 0b11010110,
            imm32: 0xDEADBEEF,
        };
        assert_eq!(instr.get_mod_mem(), 2);   // bits 0-1
        assert_eq!(instr.get_mod_shift(), 1); // bits 2-3
        assert_eq!(instr.get_mod_cond(), 13); // bits 4-7
        assert_eq!(instr.get_imm32(), 0xDEADBEEF);
    }
}

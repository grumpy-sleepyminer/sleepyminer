// AES single-round functions for RandomX.
//
// RandomX uses individual AES round functions (aesenc/aesdec), NOT full AES-128.
// These are: SubBytes + ShiftRows + MixColumns + XOR with round key.
//
// On ARM64 with crypto extensions, these map to:
//   aese + aesmc (encrypt round)
//   aesd + aesimc (decrypt round)

// AES S-Box
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

// Inverse AES S-Box
const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

// Pre-computed T-tables for AES encryption round
// T0[x] = S(x) . [02, 01, 01, 03] (little-endian column)
fn make_enc_table() -> [[u32; 256]; 4] {
    let mut t = [[0u32; 256]; 4];
    for i in 0..256 {
        let s = SBOX[i];
        let x2 = xtime(s);
        let x3 = x2 ^ s;
        t[0][i] = u32::from_le_bytes([x2, s, s, x3]);
        t[1][i] = u32::from_le_bytes([x3, x2, s, s]);
        t[2][i] = u32::from_le_bytes([s, x3, x2, s]);
        t[3][i] = u32::from_le_bytes([s, s, x3, x2]);
    }
    t
}

fn make_dec_table() -> [[u32; 256]; 4] {
    let mut t = [[0u32; 256]; 4];
    for i in 0..256 {
        let s = INV_SBOX[i];
        let x2 = xtime(s);
        let x4 = xtime(x2);
        let x8 = xtime(x4);
        let x9 = x8 ^ s;
        let xb = x8 ^ x2 ^ s;
        let xd = x8 ^ x4 ^ s;
        let xe = x8 ^ x4 ^ x2;
        t[0][i] = u32::from_le_bytes([xe, x9, xd, xb]);
        t[1][i] = u32::from_le_bytes([xb, xe, x9, xd]);
        t[2][i] = u32::from_le_bytes([xd, xb, xe, x9]);
        t[3][i] = u32::from_le_bytes([x9, xd, xb, xe]);
    }
    t
}

fn xtime(x: u8) -> u8 {
    let r = (x as u16) << 1;
    (r ^ (if r & 0x100 != 0 { 0x1b } else { 0 })) as u8
}

use std::sync::LazyLock;

static ENC_TABLE: LazyLock<[[u32; 256]; 4]> = LazyLock::new(make_enc_table);
static DEC_TABLE: LazyLock<[[u32; 256]; 4]> = LazyLock::new(make_dec_table);

/// Single AES encryption round: SubBytes + ShiftRows + MixColumns + XOR key
/// Equivalent to x86 AESENC or ARM AESE+AESMC
pub fn soft_aesenc(input: &[u8; 16], key: &[u8; 16]) -> [u8; 16] {
    let t = &*ENC_TABLE;
    let s0 = t[0][input[0] as usize] ^ t[1][input[5] as usize] ^ t[2][input[10] as usize] ^ t[3][input[15] as usize];
    let s1 = t[0][input[4] as usize] ^ t[1][input[9] as usize] ^ t[2][input[14] as usize] ^ t[3][input[3] as usize];
    let s2 = t[0][input[8] as usize] ^ t[1][input[13] as usize] ^ t[2][input[2] as usize] ^ t[3][input[7] as usize];
    let s3 = t[0][input[12] as usize] ^ t[1][input[1] as usize] ^ t[2][input[6] as usize] ^ t[3][input[11] as usize];

    let k0 = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
    let k1 = u32::from_le_bytes([key[4], key[5], key[6], key[7]]);
    let k2 = u32::from_le_bytes([key[8], key[9], key[10], key[11]]);
    let k3 = u32::from_le_bytes([key[12], key[13], key[14], key[15]]);

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&(s0 ^ k0).to_le_bytes());
    out[4..8].copy_from_slice(&(s1 ^ k1).to_le_bytes());
    out[8..12].copy_from_slice(&(s2 ^ k2).to_le_bytes());
    out[12..16].copy_from_slice(&(s3 ^ k3).to_le_bytes());
    out
}

/// Single AES decryption round: InvSubBytes + InvShiftRows + InvMixColumns + XOR key
/// Equivalent to x86 AESDEC or ARM AESD+AESIMC
pub fn soft_aesdec(input: &[u8; 16], key: &[u8; 16]) -> [u8; 16] {
    let t = &*DEC_TABLE;
    let s0 = t[0][input[0] as usize] ^ t[1][input[13] as usize] ^ t[2][input[10] as usize] ^ t[3][input[7] as usize];
    let s1 = t[0][input[4] as usize] ^ t[1][input[1] as usize] ^ t[2][input[14] as usize] ^ t[3][input[11] as usize];
    let s2 = t[0][input[8] as usize] ^ t[1][input[5] as usize] ^ t[2][input[2] as usize] ^ t[3][input[15] as usize];
    let s3 = t[0][input[12] as usize] ^ t[1][input[9] as usize] ^ t[2][input[6] as usize] ^ t[3][input[3] as usize];

    let k0 = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
    let k1 = u32::from_le_bytes([key[4], key[5], key[6], key[7]]);
    let k2 = u32::from_le_bytes([key[8], key[9], key[10], key[11]]);
    let k3 = u32::from_le_bytes([key[12], key[13], key[14], key[15]]);

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&(s0 ^ k0).to_le_bytes());
    out[4..8].copy_from_slice(&(s1 ^ k1).to_le_bytes());
    out[8..12].copy_from_slice(&(s2 ^ k2).to_le_bytes());
    out[12..16].copy_from_slice(&(s3 ^ k3).to_le_bytes());
    out
}

// Hardcoded RandomX AES constants

// fillAes1Rx4 keys (used for scratchpad fill)
// From aes_hash.cpp: #define AES_GEN_1R_KEY0 0xb4f44917, 0xdbb5552b, 0x62716609, 0x6daca553
const AES_GEN_1R_KEY0: [u8; 16] = set_vec_i128(0xb4f44917, 0xdbb5552b, 0x62716609, 0x6daca553);
const AES_GEN_1R_KEY1: [u8; 16] = set_vec_i128(0x0da1dc4e, 0x1725d378, 0x846a710d, 0x6d7caf07);
const AES_GEN_1R_KEY2: [u8; 16] = set_vec_i128(0x3e20e345, 0xf4c0794f, 0x9f947ec6, 0x3f1262f1);
const AES_GEN_1R_KEY3: [u8; 16] = set_vec_i128(0x49169154, 0x16314c88, 0xb1ba317c, 0x6aef8135);

// hashAes1Rx4 initial states (used for scratchpad hash)
// From aes_hash.cpp: #define AES_HASH_1R_STATE0 0xd7983aad, 0xcc82db47, 0x9fa856de, 0x92b52c0d
const AES_HASH_1R_STATE0: [u8; 16] = set_vec_i128(0xd7983aad, 0xcc82db47, 0x9fa856de, 0x92b52c0d);
const AES_HASH_1R_STATE1: [u8; 16] = set_vec_i128(0xace78057, 0xf59e125a, 0x15c7b798, 0x338d996e);
const AES_HASH_1R_STATE2: [u8; 16] = set_vec_i128(0xe8a07ce4, 0x5079506b, 0xae62c7d0, 0x6a770017);
const AES_HASH_1R_STATE3: [u8; 16] = set_vec_i128(0x7e994948, 0x79a10005, 0x07ad828d, 0x630a240c);

// hashAes1Rx4 extra keys for final diffusion
const AES_HASH_1R_XKEY0: [u8; 16] = set_vec_i128(0x06890201, 0x90dc56bf, 0x8b24949f, 0xf6fa8389);
const AES_HASH_1R_XKEY1: [u8; 16] = set_vec_i128(0xed18f99b, 0xee1043c6, 0x51f4e03c, 0x61b263d1);

/// Mirrors _mm_set_epi32(i3, i2, i1, i0) -> little-endian byte layout [i0, i1, i2, i3]
const fn set_vec_i128(i3: u32, i2: u32, i1: u32, i0: u32) -> [u8; 16] {
    let b0 = i0.to_le_bytes();
    let b1 = i1.to_le_bytes();
    let b2 = i2.to_le_bytes();
    let b3 = i3.to_le_bytes();
    [
        b0[0], b0[1], b0[2], b0[3],
        b1[0], b1[1], b1[2], b1[3],
        b2[0], b2[1], b2[2], b2[3],
        b3[0], b3[1], b3[2], b3[3],
    ]
}

/// fillAes1Rx4: Fill buffer with pseudorandom data using single AES rounds.
/// State is 64 bytes (4 x 128-bit), keys are hardcoded constants.
/// Used for scratchpad initialization.
pub fn fill_aes_1rx4(state: &mut [u8; 64], output: &mut [u8]) {
    assert!(output.len() % 64 == 0);

    let mut s0: [u8; 16] = state[0..16].try_into().unwrap();
    let mut s1: [u8; 16] = state[16..32].try_into().unwrap();
    let mut s2: [u8; 16] = state[32..48].try_into().unwrap();
    let mut s3: [u8; 16] = state[48..64].try_into().unwrap();

    let mut offset = 0;
    while offset < output.len() {
        s0 = soft_aesdec(&s0, &AES_GEN_1R_KEY0);
        s1 = soft_aesenc(&s1, &AES_GEN_1R_KEY1);
        s2 = soft_aesdec(&s2, &AES_GEN_1R_KEY2);
        s3 = soft_aesenc(&s3, &AES_GEN_1R_KEY3);

        output[offset..offset + 16].copy_from_slice(&s0);
        output[offset + 16..offset + 32].copy_from_slice(&s1);
        output[offset + 32..offset + 48].copy_from_slice(&s2);
        output[offset + 48..offset + 64].copy_from_slice(&s3);

        offset += 64;
    }

    state[0..16].copy_from_slice(&s0);
    state[16..32].copy_from_slice(&s1);
    state[32..48].copy_from_slice(&s2);
    state[48..64].copy_from_slice(&s3);
}

/// hashAes1Rx4: Hash input data using single AES rounds.
/// Input is treated as round keys, states are hardcoded.
/// Used for scratchpad finalization.
/// Output: 64 bytes written to `hash`.
pub fn hash_aes_1rx4(input: &[u8], hash: &mut [u8; 64]) {
    assert!(input.len() % 64 == 0);

    let mut s0 = AES_HASH_1R_STATE0;
    let mut s1 = AES_HASH_1R_STATE1;
    let mut s2 = AES_HASH_1R_STATE2;
    let mut s3 = AES_HASH_1R_STATE3;

    let mut offset = 0;
    while offset < input.len() {
        let in0: [u8; 16] = input[offset..offset + 16].try_into().unwrap();
        let in1: [u8; 16] = input[offset + 16..offset + 32].try_into().unwrap();
        let in2: [u8; 16] = input[offset + 32..offset + 48].try_into().unwrap();
        let in3: [u8; 16] = input[offset + 48..offset + 64].try_into().unwrap();

        s0 = soft_aesenc(&s0, &in0);
        s1 = soft_aesdec(&s1, &in1);
        s2 = soft_aesenc(&s2, &in2);
        s3 = soft_aesdec(&s3, &in3);

        offset += 64;
    }

    // Two extra rounds for full diffusion
    s0 = soft_aesenc(&s0, &AES_HASH_1R_XKEY0);
    s1 = soft_aesdec(&s1, &AES_HASH_1R_XKEY0);
    s2 = soft_aesenc(&s2, &AES_HASH_1R_XKEY0);
    s3 = soft_aesdec(&s3, &AES_HASH_1R_XKEY0);

    s0 = soft_aesenc(&s0, &AES_HASH_1R_XKEY1);
    s1 = soft_aesdec(&s1, &AES_HASH_1R_XKEY1);
    s2 = soft_aesenc(&s2, &AES_HASH_1R_XKEY1);
    s3 = soft_aesdec(&s3, &AES_HASH_1R_XKEY1);

    hash[0..16].copy_from_slice(&s0);
    hash[16..32].copy_from_slice(&s1);
    hash[32..48].copy_from_slice(&s2);
    hash[48..64].copy_from_slice(&s3);
}

/// fillAes4Rx4: Generate RandomX programs using 4 AES rounds per block.
/// Used for program generation between hash rounds.
pub fn fill_aes_4rx4(state: &mut [u8; 64], output: &mut [u8]) {
    assert!(output.len() % 64 == 0);

    // fillAes4Rx4 uses 8 keys (from RandomX_ConfigurationMonero.fillAes4Rx4_Key)
    // rx_set_int_vec_i128(a,b,c,d) = _mm_set_epi32(a,b,c,d) = memory layout [d,c,b,a]
    // Monero default: key[4..7] = key[0..3]
    const KEY0: [u8; 16] = set_vec_i128(0xcf359e95, 0x141f82b7, 0x7ffbe4a6, 0xf890465d);
    const KEY1: [u8; 16] = set_vec_i128(0x6741ffdc, 0xbd5c5ac3, 0xfee8278a, 0x6a55c450);
    const KEY2: [u8; 16] = set_vec_i128(0x3d324aac, 0xa7279ad2, 0xd524fde4, 0x114c47a4);
    const KEY3: [u8; 16] = set_vec_i128(0x76f6db08, 0x42d3dbd9, 0x99a9aeff, 0x810c3a2a);
    const KEY4: [u8; 16] = KEY0;
    const KEY5: [u8; 16] = KEY1;
    const KEY6: [u8; 16] = KEY2;
    const KEY7: [u8; 16] = KEY3;

    let mut s0: [u8; 16] = state[0..16].try_into().unwrap();
    let mut s1: [u8; 16] = state[16..32].try_into().unwrap();
    let mut s2: [u8; 16] = state[32..48].try_into().unwrap();
    let mut s3: [u8; 16] = state[48..64].try_into().unwrap();

    let mut offset = 0;
    while offset < output.len() {
        // 4 AES rounds per block
        s0 = soft_aesdec(&s0, &KEY0);
        s1 = soft_aesenc(&s1, &KEY0);
        s2 = soft_aesdec(&s2, &KEY4);
        s3 = soft_aesenc(&s3, &KEY4);

        s0 = soft_aesdec(&s0, &KEY1);
        s1 = soft_aesenc(&s1, &KEY1);
        s2 = soft_aesdec(&s2, &KEY5);
        s3 = soft_aesenc(&s3, &KEY5);

        s0 = soft_aesdec(&s0, &KEY2);
        s1 = soft_aesenc(&s1, &KEY2);
        s2 = soft_aesdec(&s2, &KEY6);
        s3 = soft_aesenc(&s3, &KEY6);

        s0 = soft_aesdec(&s0, &KEY3);
        s1 = soft_aesenc(&s1, &KEY3);
        s2 = soft_aesdec(&s2, &KEY7);
        s3 = soft_aesenc(&s3, &KEY7);

        output[offset..offset + 16].copy_from_slice(&s0);
        output[offset + 16..offset + 32].copy_from_slice(&s1);
        output[offset + 32..offset + 48].copy_from_slice(&s2);
        output[offset + 48..offset + 64].copy_from_slice(&s3);

        offset += 64;
    }

    state[0..16].copy_from_slice(&s0);
    state[16..32].copy_from_slice(&s1);
    state[32..48].copy_from_slice(&s2);
    state[48..64].copy_from_slice(&s3);
}

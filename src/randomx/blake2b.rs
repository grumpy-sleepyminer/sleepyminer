use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;

/// Compute Blake2b hash with variable output length.
pub fn blake2b(input: &[u8], output_len: usize) -> Vec<u8> {
    let mut hasher = Blake2bVar::new(output_len).expect("Invalid output length");
    hasher.update(input);
    let mut result = vec![0u8; output_len];
    hasher.finalize_variable(&mut result).expect("Finalization failed");
    result
}

/// Compute Blake2b-256 hash.
pub fn blake2b_256(input: &[u8]) -> [u8; 32] {
    let result = blake2b(input, 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Compute Blake2b-512 hash (used for RandomX initial hash).
pub fn blake2b_512(input: &[u8]) -> [u8; 64] {
    let result = blake2b(input, 64);
    let mut out = [0u8; 64];
    out.copy_from_slice(&result);
    out
}

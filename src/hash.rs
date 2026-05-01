//! Cryptographic hash utilities shared across the crate.
//!
//! Used by safe-output tools to record and verify file integrity between
//! Stage 1 (MCP, in-sandbox) and Stage 3 (executor, outside sandbox).

use sha2::{Digest, Sha256};

/// Compute the SHA-256 hex digest of a byte slice.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_empty() {
        // SHA-256 of empty input is well-known.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_hello() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}

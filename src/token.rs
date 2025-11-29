//! API Token generation and validation
//!
//! Tokens are formatted as: pg_<random_32_bytes_hex>
//! The hash is SHA-256 of the full token string

use rand::Rng;
use sha2::{Digest, Sha256};

/// Token prefix for identification
pub const TOKEN_PREFIX: &str = "pg_";

/// Generate a new random token
/// Returns (full_token, token_hash, token_prefix)
pub fn generate_token() -> (String, String, String) {
    let mut rng = rand::thread_rng();
    let random_bytes: [u8; 32] = rng.r#gen();
    let random_hex = hex::encode(random_bytes);

    let full_token = format!("{}{}", TOKEN_PREFIX, random_hex);
    let token_hash = hash_token(&full_token);
    let prefix = full_token[..8.min(full_token.len())].to_string();

    (full_token, token_hash, prefix)
}

/// Hash a token using SHA-256
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Validate a token format
pub fn is_valid_format(token: &str) -> bool {
    token.starts_with(TOKEN_PREFIX) && token.len() == TOKEN_PREFIX.len() + 64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_token() {
        let (token, hash, prefix) = generate_token();

        // Token should start with prefix
        assert!(token.starts_with(TOKEN_PREFIX));

        // Token should be 67 chars (3 prefix + 64 hex)
        assert_eq!(token.len(), 67);

        // Hash should be 64 chars (SHA-256 hex)
        assert_eq!(hash.len(), 64);

        // Prefix should be first 8 chars
        assert_eq!(prefix, &token[..8]);

        // Hashing same token should give same result
        assert_eq!(hash_token(&token), hash);
    }

    #[test]
    fn test_hash_consistency() {
        let token = "pg_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_is_valid_format() {
        // Valid
        assert!(is_valid_format(
            "pg_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));

        // Invalid - wrong prefix
        assert!(!is_valid_format(
            "xx_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));

        // Invalid - too short
        assert!(!is_valid_format("pg_0123456789abcdef"));

        // Invalid - too long
        assert!(!is_valid_format(
            "pg_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef00"
        ));
    }
}

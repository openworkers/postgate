//! Token-based authentication for postgate
//!
//! Tokens are formatted as: pg_<random_64_hex_chars>
//! They are validated by hashing and comparing with stored hash

use std::collections::HashSet;
use thiserror::Error;
use uuid::Uuid;

use crate::config::SqlOperation;
use crate::token::{hash_token, is_valid_format};

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Missing authorization header")]
    MissingHeader,

    #[error("Invalid authorization header format")]
    InvalidFormat,

    #[error("Invalid token format")]
    InvalidTokenFormat,

    #[error("Invalid or expired token")]
    InvalidToken,
}

/// Token info returned after validation
#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub database_id: Uuid,
    pub token_id: Uuid,
    pub allowed_operations: HashSet<SqlOperation>,
}

/// Extract token from Authorization header
/// Supports both "Bearer <token>" and plain "<token>" formats
pub fn extract_token(auth_header: Option<&str>) -> Result<String, AuthError> {
    let header = auth_header.ok_or(AuthError::MissingHeader)?;

    // Support both "Bearer pg_xxx" and "pg_xxx" formats
    let token = header.strip_prefix("Bearer ").unwrap_or(header).trim();

    if !is_valid_format(token) {
        return Err(AuthError::InvalidTokenFormat);
    }

    Ok(token.to_string())
}

/// Compute the hash of a token for database lookup
pub fn compute_token_hash(token: &str) -> String {
    hash_token(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::generate_token;

    #[test]
    fn test_extract_token_bearer() {
        let (token, _, _) = generate_token();
        let header = format!("Bearer {}", token);

        let result = extract_token(Some(&header));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), token);
    }

    #[test]
    fn test_extract_token_plain() {
        let (token, _, _) = generate_token();

        let result = extract_token(Some(&token));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), token);
    }

    #[test]
    fn test_extract_token_missing() {
        let result = extract_token(None);
        assert!(matches!(result, Err(AuthError::MissingHeader)));
    }

    #[test]
    fn test_extract_token_invalid_format() {
        let result = extract_token(Some("Bearer invalid_token"));
        assert!(matches!(result, Err(AuthError::InvalidTokenFormat)));
    }

    #[test]
    fn test_compute_token_hash() {
        let (token, expected_hash, _) = generate_token();
        let computed_hash = compute_token_hash(&token);
        assert_eq!(computed_hash, expected_hash);
    }
}

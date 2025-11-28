use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Missing authorization header")]
    MissingHeader,

    #[error("Invalid authorization header format")]
    InvalidFormat,

    #[error("Invalid token: {0}")]
    InvalidToken(#[from] jsonwebtoken::errors::Error),

    #[error("Invalid database_id in token")]
    InvalidDatabaseId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject - the database UUID
    pub sub: String,
    /// Expiration time (Unix timestamp)
    pub exp: usize,
}

pub struct JwtValidator {
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtValidator {
    pub fn new(secret: &str) -> Self {
        Self {
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            validation: Validation::default(),
        }
    }

    pub fn validate(&self, token: &str) -> Result<Uuid, AuthError> {
        let token_data = decode::<Claims>(token, &self.decoding_key, &self.validation)?;

        Uuid::parse_str(&token_data.claims.sub).map_err(|_| AuthError::InvalidDatabaseId)
    }
}

/// Extract and validate JWT from Authorization header
pub fn extract_database_id(
    auth_header: Option<&str>,
    validator: &JwtValidator,
) -> Result<Uuid, AuthError> {
    let header = auth_header.ok_or(AuthError::MissingHeader)?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or(AuthError::InvalidFormat)?;

    validator.validate(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    fn create_test_token(secret: &str, database_id: &Uuid, exp: usize) -> String {
        let claims = Claims {
            sub: database_id.to_string(),
            exp,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn test_valid_token() {
        let secret = "test_secret";
        let database_id = Uuid::new_v4();
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;

        let token = create_test_token(secret, &database_id, exp);
        let validator = JwtValidator::new(secret);

        let result = validator.validate(&token);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), database_id);
    }

    #[test]
    fn test_invalid_secret() {
        let database_id = Uuid::new_v4();
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;

        let token = create_test_token("secret1", &database_id, exp);
        let validator = JwtValidator::new("secret2");

        let result = validator.validate(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_token() {
        let secret = "test_secret";
        let database_id = Uuid::new_v4();
        let exp = (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp() as usize;

        let token = create_test_token(secret, &database_id, exp);
        let validator = JwtValidator::new(secret);

        let result = validator.validate(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_database_id() {
        let secret = "test_secret";
        let database_id = Uuid::new_v4();
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;

        let token = create_test_token(secret, &database_id, exp);
        let validator = JwtValidator::new(secret);

        let header = format!("Bearer {}", token);
        let result = extract_database_id(Some(&header), &validator);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), database_id);
    }

    #[test]
    fn test_missing_header() {
        let validator = JwtValidator::new("secret");
        let result = extract_database_id(None, &validator);
        assert!(matches!(result, Err(AuthError::MissingHeader)));
    }

    #[test]
    fn test_invalid_format() {
        let validator = JwtValidator::new("secret");
        let result = extract_database_id(Some("Basic xyz"), &validator);
        assert!(matches!(result, Err(AuthError::InvalidFormat)));
    }
}

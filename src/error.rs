use actix_web::{HttpResponse, ResponseError};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::executor::ExecutorError;
use crate::parser::ParseError;

#[derive(Debug, Error)]
pub enum PostgateError {
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("Execution error: {0}")]
    Executor(#[from] ExecutorError),

    #[error("Database not found: {0}")]
    DatabaseNotFound(Uuid),

    #[error("Missing authorization header")]
    MissingAuth,

    #[error("Invalid authorization header")]
    InvalidAuth,

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: &'static str,
}

impl ResponseError for PostgateError {
    fn error_response(&self) -> HttpResponse {
        let (status, code) = match self {
            PostgateError::Parse(_) => (actix_web::http::StatusCode::BAD_REQUEST, "PARSE_ERROR"),
            PostgateError::Executor(ExecutorError::Timeout) => {
                (actix_web::http::StatusCode::GATEWAY_TIMEOUT, "TIMEOUT")
            }
            PostgateError::Executor(ExecutorError::RowLimitExceeded(_)) => (
                actix_web::http::StatusCode::BAD_REQUEST,
                "ROW_LIMIT_EXCEEDED",
            ),
            PostgateError::Executor(_) => (
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_ERROR",
            ),
            PostgateError::DatabaseNotFound(_) => {
                (actix_web::http::StatusCode::NOT_FOUND, "DATABASE_NOT_FOUND")
            }
            PostgateError::MissingAuth | PostgateError::InvalidAuth => {
                (actix_web::http::StatusCode::UNAUTHORIZED, "UNAUTHORIZED")
            }
            PostgateError::Internal(_) => (
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
            ),
        };

        HttpResponse::build(status).json(ErrorResponse {
            error: self.to_string(),
            code,
        })
    }
}

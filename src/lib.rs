pub mod auth;
pub mod config;
pub mod error;
pub mod executor;
pub mod parser;
pub mod store;
pub mod token;

#[cfg(feature = "server")]
pub mod server;

// Re-export main types for convenience
pub use config::{DatabaseBackend, DatabaseConfig, QueryRules, SqlOperation};
pub use executor::{ExecutorError, ExecutorPool, QueryRequest, QueryResponse};
pub use parser::{ParseError, ParsedQuery, parse_and_validate};

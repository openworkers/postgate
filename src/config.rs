use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DatabaseBackend {
    Schema { schema_name: String },
    Dedicated { connection_string: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub name: String,
    pub backend: DatabaseBackend,
    pub rules: QueryRules,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryRules {
    #[serde(default)]
    pub allowed_operations: HashSet<SqlOperation>,
    #[serde(default)]
    pub allowed_tables: Option<HashSet<String>>,
    #[serde(default)]
    pub denied_tables: HashSet<String>,
    #[serde(default = "default_row_limit")]
    pub max_rows: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_row_limit() -> u32 {
    1000
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SqlOperation {
    Select,
    Insert,
    Update,
    Delete,
}

impl SqlOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            SqlOperation::Select => "SELECT",
            SqlOperation::Insert => "INSERT",
            SqlOperation::Update => "UPDATE",
            SqlOperation::Delete => "DELETE",
        }
    }
}

impl std::fmt::Display for SqlOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

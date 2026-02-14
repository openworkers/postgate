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
    pub max_body_size_mb: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
            max_body_size_mb: 10,
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
    pub name: String,
    pub backend: DatabaseBackend,
    pub max_rows: i32,
}

/// Rules used for parsing/validating queries
/// allowed_operations comes from token, tables rules are system-level
#[derive(Debug, Clone, Default)]
pub struct QueryRules {
    pub allowed_operations: HashSet<SqlOperation>,
    pub allowed_tables: Option<HashSet<String>>,
    pub denied_tables: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SqlOperation {
    Select,
    Insert,
    Update,
    Delete,
    Create,
    Alter,
    Drop,
}

impl SqlOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            SqlOperation::Select => "SELECT",
            SqlOperation::Insert => "INSERT",
            SqlOperation::Update => "UPDATE",
            SqlOperation::Delete => "DELETE",
            SqlOperation::Create => "CREATE",
            SqlOperation::Alter => "ALTER",
            SqlOperation::Drop => "DROP",
        }
    }

    /// Returns true for DDL operations (CREATE, ALTER, DROP)
    /// DDL statements don't return rows and need special handling
    pub fn is_ddl(&self) -> bool {
        matches!(
            self,
            SqlOperation::Create | SqlOperation::Alter | SqlOperation::Drop
        )
    }
}

impl std::fmt::Display for SqlOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Token permission matching SQL operations
/// Maps directly to database allowed_operations array
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TokenPermission {
    Select,
    Insert,
    Update,
    Delete,
    Create,
    Alter,
    Drop,
}

impl TokenPermission {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenPermission::Select => "SELECT",
            TokenPermission::Insert => "INSERT",
            TokenPermission::Update => "UPDATE",
            TokenPermission::Delete => "DELETE",
            TokenPermission::Create => "CREATE",
            TokenPermission::Alter => "ALTER",
            TokenPermission::Drop => "DROP",
        }
    }

    /// Default permissions (DML only)
    pub fn default_set() -> &'static [TokenPermission] {
        &[
            TokenPermission::Select,
            TokenPermission::Insert,
            TokenPermission::Update,
            TokenPermission::Delete,
        ]
    }

    /// Tenant permissions (DML + DDL)
    pub fn tenant_set() -> &'static [TokenPermission] {
        &[
            TokenPermission::Select,
            TokenPermission::Insert,
            TokenPermission::Update,
            TokenPermission::Delete,
            TokenPermission::Create,
            TokenPermission::Alter,
            TokenPermission::Drop,
        ]
    }
}

impl std::fmt::Display for TokenPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

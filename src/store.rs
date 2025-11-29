use sqlx::postgres::PgPool;
use std::collections::HashSet;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::TokenInfo;
use crate::config::{DatabaseBackend, DatabaseConfig, SqlOperation, TokenPermission};
use crate::token::generate_token;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Database not found: {0}")]
    NotFound(Uuid),

    #[error("Invalid backend type: {0}")]
    InvalidBackendType(String),

    #[error("Token not found")]
    TokenNotFound,
}

pub struct Store {
    pool: PgPool,
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_database(&self, id: Uuid) -> Result<DatabaseConfig, StoreError> {
        let row = sqlx::query!(
            r#"
            SELECT id, name, backend_type, schema_name, connection_string, max_rows
            FROM postgate_databases
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound(id))?;

        let backend = match row.backend_type.as_str() {
            "schema" => DatabaseBackend::Schema {
                schema_name: row.schema_name.unwrap_or_default(),
            },
            "dedicated" => DatabaseBackend::Dedicated {
                connection_string: row.connection_string.unwrap_or_default(),
            },
            other => return Err(StoreError::InvalidBackendType(other.to_string())),
        };

        Ok(DatabaseConfig {
            id: row.id,
            name: row.name,
            backend,
            max_rows: row.max_rows,
        })
    }

    pub async fn create_database(
        &self,
        name: &str,
        backend: &DatabaseBackend,
        max_rows: i32,
    ) -> Result<DatabaseConfig, StoreError> {
        let (backend_type, schema_name, connection_string) = match backend {
            DatabaseBackend::Schema { schema_name } => ("schema", Some(schema_name.clone()), None),
            DatabaseBackend::Dedicated { connection_string } => {
                ("dedicated", None, Some(connection_string.clone()))
            }
        };

        let row = sqlx::query!(
            r#"
            INSERT INTO postgate_databases (name, backend_type, schema_name, connection_string, max_rows)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
            name,
            backend_type,
            schema_name,
            connection_string,
            max_rows
        )
        .fetch_one(&self.pool)
        .await?;

        // Create the schema if needed
        if let DatabaseBackend::Schema { schema_name } = backend {
            sqlx::query(&format!(
                "CREATE SCHEMA IF NOT EXISTS \"{}\"",
                schema_name.replace('"', "\"\"")
            ))
            .execute(&self.pool)
            .await?;
        }

        Ok(DatabaseConfig {
            id: row.id,
            name: name.to_string(),
            backend: backend.clone(),
            max_rows,
        })
    }

    pub async fn delete_database(&self, id: Uuid) -> Result<(), StoreError> {
        // Get the database first to know if we need to drop a schema
        let db = self.get_database(id).await?;

        // Delete from table
        sqlx::query!("DELETE FROM postgate_databases WHERE id = $1", id)
            .execute(&self.pool)
            .await?;

        // Drop schema if it was a schema backend
        if let DatabaseBackend::Schema { schema_name } = db.backend {
            sqlx::query(&format!(
                "DROP SCHEMA IF EXISTS \"{}\" CASCADE",
                schema_name.replace('"', "\"\"")
            ))
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    pub async fn list_databases(&self) -> Result<Vec<DatabaseConfig>, StoreError> {
        let rows = sqlx::query!(
            r#"
            SELECT id, name, backend_type, schema_name, connection_string, max_rows
            FROM postgate_databases
            ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut databases = Vec::new();
        for row in rows {
            let backend = match row.backend_type.as_str() {
                "schema" => DatabaseBackend::Schema {
                    schema_name: row.schema_name.unwrap_or_default(),
                },
                "dedicated" => DatabaseBackend::Dedicated {
                    connection_string: row.connection_string.unwrap_or_default(),
                },
                _ => continue,
            };

            databases.push(DatabaseConfig {
                id: row.id,
                name: row.name,
                backend,
                max_rows: row.max_rows,
            });
        }

        Ok(databases)
    }

    // ============ Token Methods ============

    /// Create a new token for a database
    /// Returns (token_id, full_token) - the full token is only returned once!
    pub async fn create_token(
        &self,
        database_id: Uuid,
        name: &str,
        permissions: &[TokenPermission],
    ) -> Result<(Uuid, String), StoreError> {
        let (full_token, token_hash, token_prefix) = generate_token();
        let ops_vec: Vec<String> = permissions.iter().map(|p| p.as_str().to_string()).collect();

        let token_id: Uuid = sqlx::query_scalar!(
            r#"
            INSERT INTO postgate_tokens (database_id, name, token_hash, token_prefix, allowed_operations)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
            database_id,
            name,
            token_hash,
            token_prefix,
            &ops_vec
        )
        .fetch_one(&self.pool)
        .await?;

        Ok((token_id, full_token))
    }

    /// Validate a token by its hash and return the associated database_id and allowed_operations
    pub async fn validate_token(&self, token_hash: &str) -> Result<TokenInfo, StoreError> {
        let row = sqlx::query!(
            r#"
            SELECT t.id, t.database_id, t.allowed_operations
            FROM postgate_tokens t
            WHERE t.token_hash = $1
            "#,
            token_hash
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::TokenNotFound)?;

        // Update last_used_at (fire and forget)
        let pool = self.pool.clone();
        let token_id = row.id;
        tokio::spawn(async move {
            let _ = sqlx::query!(
                "UPDATE postgate_tokens SET last_used_at = NOW() WHERE id = $1",
                token_id
            )
            .execute(&pool)
            .await;
        });

        // Parse allowed_operations from text[] to HashSet<SqlOperation>
        let allowed_operations: HashSet<SqlOperation> = row
            .allowed_operations
            .iter()
            .filter_map(|op| match op.as_str() {
                "SELECT" => Some(SqlOperation::Select),
                "INSERT" => Some(SqlOperation::Insert),
                "UPDATE" => Some(SqlOperation::Update),
                "DELETE" => Some(SqlOperation::Delete),
                "CREATE" => Some(SqlOperation::Create),
                "ALTER" => Some(SqlOperation::Alter),
                "DROP" => Some(SqlOperation::Drop),
                _ => None,
            })
            .collect();

        Ok(TokenInfo {
            database_id: row.database_id,
            token_id: row.id,
            allowed_operations,
        })
    }

    /// Delete a token by ID
    pub async fn delete_token(&self, token_id: Uuid) -> Result<(), StoreError> {
        sqlx::query!("DELETE FROM postgate_tokens WHERE id = $1", token_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete all tokens for a database
    pub async fn delete_tokens_for_database(&self, database_id: Uuid) -> Result<(), StoreError> {
        sqlx::query!(
            "DELETE FROM postgate_tokens WHERE database_id = $1",
            database_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// List tokens for a database (without the actual token values)
    pub async fn list_tokens(&self, database_id: Uuid) -> Result<Vec<TokenListItem>, StoreError> {
        let rows = sqlx::query!(
            r#"
            SELECT id, name, token_prefix, created_at, last_used_at
            FROM postgate_tokens
            WHERE database_id = $1
            ORDER BY created_at DESC
            "#,
            database_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| TokenListItem {
                id: r.id,
                name: r.name,
                token_prefix: r.token_prefix,
                created_at: r.created_at,
                last_used_at: r.last_used_at,
            })
            .collect())
    }
}

/// Token info for listing (without the secret)
#[derive(Debug, Clone)]
pub struct TokenListItem {
    pub id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub fn generate_schema_name(db_name: &str) -> String {
    // Create a safe schema name: db_<uuid>_<sanitized_name>
    let uuid_short = &Uuid::new_v4().to_string()[..8];
    let safe_name: String = db_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .take(50)
        .collect();
    format!("db_{}_{}", uuid_short, safe_name.to_lowercase())
}

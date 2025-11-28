use sqlx::postgres::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::config::{DatabaseBackend, DatabaseConfig, QueryRules};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Database not found: {0}")]
    NotFound(Uuid),

    #[error("Invalid backend type: {0}")]
    InvalidBackendType(String),
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
            SELECT id, user_id, name, backend_type, schema_name, connection_string, rules
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

        let rules: QueryRules = serde_json::from_value(row.rules).unwrap_or_default();

        Ok(DatabaseConfig {
            id: row.id,
            user_id: row.user_id,
            name: row.name,
            backend,
            rules,
        })
    }

    pub async fn create_database(
        &self,
        user_id: Uuid,
        name: &str,
        backend: &DatabaseBackend,
        rules: &QueryRules,
    ) -> Result<DatabaseConfig, StoreError> {
        let (backend_type, schema_name, connection_string) = match backend {
            DatabaseBackend::Schema { schema_name } => ("schema", Some(schema_name.clone()), None),
            DatabaseBackend::Dedicated { connection_string } => {
                ("dedicated", None, Some(connection_string.clone()))
            }
        };

        let rules_json = serde_json::to_value(rules).unwrap_or_default();

        let row = sqlx::query!(
            r#"
            INSERT INTO postgate_databases (user_id, name, backend_type, schema_name, connection_string, rules)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
            user_id,
            name,
            backend_type,
            schema_name,
            connection_string,
            rules_json
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
            user_id,
            name: name.to_string(),
            backend: backend.clone(),
            rules: rules.clone(),
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

    pub async fn list_databases_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<DatabaseConfig>, StoreError> {
        let rows = sqlx::query!(
            r#"
            SELECT id, user_id, name, backend_type, schema_name, connection_string, rules
            FROM postgate_databases
            WHERE user_id = $1
            ORDER BY created_at DESC
            "#,
            user_id
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

            let rules: QueryRules = serde_json::from_value(row.rules).unwrap_or_default();

            databases.push(DatabaseConfig {
                id: row.id,
                user_id: row.user_id,
                name: row.name,
                backend,
                rules,
            });
        }

        Ok(databases)
    }
}

pub fn generate_schema_name(user_id: Uuid, db_name: &str) -> String {
    // Create a safe schema name: db_<short_user_id>_<sanitized_name>
    let short_id = &user_id.to_string()[..8];
    let safe_name: String = db_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .take(50)
        .collect();
    format!("db_{}_{}", short_id, safe_name.to_lowercase())
}

pub fn default_rules() -> QueryRules {
    use crate::config::SqlOperation;

    QueryRules {
        allowed_operations: [
            SqlOperation::Select,
            SqlOperation::Insert,
            SqlOperation::Update,
            SqlOperation::Delete,
        ]
        .into_iter()
        .collect(),
        allowed_tables: None,
        denied_tables: Default::default(),
        max_rows: 1000,
        timeout_seconds: 30,
    }
}

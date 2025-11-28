use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow, PgTypeInfo};
use sqlx::{Column, Row, TypeInfo};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Query timeout")]
    Timeout,

    #[error("Row limit exceeded (max: {0})")]
    RowLimitExceeded(u32),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryRequest {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<JsonValue>,
}

#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub rows: Vec<HashMap<String, JsonValue>>,
    pub row_count: usize,
}

pub struct Executor {
    pool: PgPool,
}

impl Executor {
    pub async fn new(connection_string: &str) -> Result<Self, ExecutorError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(connection_string)
            .await?;

        Ok(Self { pool })
    }

    pub async fn execute(
        &self,
        request: &QueryRequest,
        max_rows: u32,
        timeout_seconds: u64,
    ) -> Result<QueryResponse, ExecutorError> {
        let timeout = Duration::from_secs(timeout_seconds);

        let result = tokio::time::timeout(timeout, self.execute_query(request, max_rows)).await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(ExecutorError::Timeout),
        }
    }

    async fn execute_query(
        &self,
        request: &QueryRequest,
        max_rows: u32,
    ) -> Result<QueryResponse, ExecutorError> {
        let mut query = sqlx::query(&request.sql);

        for param in &request.params {
            query = bind_json_value(query, param);
        }

        let rows: Vec<PgRow> = query.fetch_all(&self.pool).await?;

        if rows.len() > max_rows as usize {
            return Err(ExecutorError::RowLimitExceeded(max_rows));
        }

        let row_count = rows.len();
        let rows = rows.into_iter().map(row_to_json).collect();

        Ok(QueryResponse { rows, row_count })
    }
}

fn bind_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &'q JsonValue,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match value {
        JsonValue::Null => query.bind(None::<String>),
        JsonValue::Bool(b) => query.bind(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i)
            } else if let Some(f) = n.as_f64() {
                query.bind(f)
            } else {
                query.bind(n.to_string())
            }
        }
        JsonValue::String(s) => query.bind(s.as_str()),
        JsonValue::Array(_) | JsonValue::Object(_) => query.bind(value),
    }
}

fn row_to_json(row: PgRow) -> HashMap<String, JsonValue> {
    let mut map = HashMap::new();

    for column in row.columns() {
        let name = column.name().to_string();
        let value = get_column_value(&row, column.ordinal(), column.type_info());
        map.insert(name, value);
    }

    map
}

fn get_column_value(row: &PgRow, idx: usize, type_info: &PgTypeInfo) -> JsonValue {
    let type_name = type_info.name();

    match type_name {
        "BOOL" => row
            .try_get::<Option<bool>, _>(idx)
            .ok()
            .flatten()
            .map(JsonValue::Bool)
            .unwrap_or(JsonValue::Null),
        "INT2" => row
            .try_get::<Option<i16>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::Number(v.into()))
            .unwrap_or(JsonValue::Null),
        "INT4" => row
            .try_get::<Option<i32>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::Number(v.into()))
            .unwrap_or(JsonValue::Null),
        "INT8" => row
            .try_get::<Option<i64>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::Number(v.into()))
            .unwrap_or(JsonValue::Null),
        "FLOAT4" => row
            .try_get::<Option<f32>, _>(idx)
            .ok()
            .flatten()
            .and_then(|v| serde_json::Number::from_f64(v as f64))
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        "FLOAT8" => row
            .try_get::<Option<f64>, _>(idx)
            .ok()
            .flatten()
            .and_then(serde_json::Number::from_f64)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        "TEXT" | "VARCHAR" | "CHAR" | "NAME" | "BPCHAR" => row
            .try_get::<Option<String>, _>(idx)
            .ok()
            .flatten()
            .map(JsonValue::String)
            .unwrap_or(JsonValue::Null),
        "UUID" => row
            .try_get::<Option<uuid::Uuid>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::String(v.to_string()))
            .unwrap_or(JsonValue::Null),
        "TIMESTAMPTZ" => row
            .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::String(v.to_rfc3339()))
            .unwrap_or(JsonValue::Null),
        "TIMESTAMP" => row
            .try_get::<Option<chrono::NaiveDateTime>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::String(v.to_string()))
            .unwrap_or(JsonValue::Null),
        "DATE" => row
            .try_get::<Option<chrono::NaiveDate>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::String(v.to_string()))
            .unwrap_or(JsonValue::Null),
        "TIME" => row
            .try_get::<Option<chrono::NaiveTime>, _>(idx)
            .ok()
            .flatten()
            .map(|v| JsonValue::String(v.to_string()))
            .unwrap_or(JsonValue::Null),
        "JSON" | "JSONB" => row
            .try_get::<Option<JsonValue>, _>(idx)
            .ok()
            .flatten()
            .unwrap_or(JsonValue::Null),
        _ => {
            // Fallback: try as string
            row.try_get::<Option<String>, _>(idx)
                .ok()
                .flatten()
                .map(JsonValue::String)
                .unwrap_or(JsonValue::Null)
        }
    }
}

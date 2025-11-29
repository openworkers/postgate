use actix_web::{HttpRequest, HttpResponse, web};

use crate::auth::{compute_token_hash, extract_token};
use crate::config::Config;
use crate::error::PostgateError;
use crate::executor::{ExecutorPool, QueryRequest, QueryResponse};
use crate::parser::parse_and_validate;
use crate::store::Store;

pub struct AppState {
    pub config: Config,
    pub executor_pool: ExecutorPool,
    pub store: Store,
}

impl AppState {
    pub fn new(config: Config, executor_pool: ExecutorPool, store: Store) -> Self {
        Self {
            config,
            executor_pool,
            store,
        }
    }
}

/// Default query timeout in seconds
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

pub async fn query_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, PostgateError> {
    // Extract and validate token
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok());

    let token = extract_token(auth_header).map_err(|e| match e {
        crate::auth::AuthError::MissingHeader => PostgateError::MissingAuth,
        _ => PostgateError::InvalidAuth,
    })?;

    let token_hash = compute_token_hash(&token);

    let token_info = state
        .store
        .validate_token(&token_hash)
        .await
        .map_err(|_| PostgateError::InvalidAuth)?;

    // Load database config from store
    let db_config = state
        .store
        .get_database(token_info.database_id)
        .await
        .map_err(|_| PostgateError::DatabaseNotFound(token_info.database_id))?;

    // Parse and validate SQL using allowed_operations from token
    let parsed = parse_and_validate(&body.sql, &token_info.allowed_operations)?;

    // Execute query with max_rows from database config
    let response: QueryResponse = state
        .executor_pool
        .execute(
            token_info.database_id,
            &db_config.backend,
            &body,
            db_config.max_rows as u32,
            DEFAULT_TIMEOUT_SECONDS,
            parsed.operation.is_ddl(),
        )
        .await
        .map_err(PostgateError::Executor)?;

    Ok(HttpResponse::Ok().json(response))
}

pub async fn health_handler() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok"
    }))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health_handler))
        .route("/query", web::post().to(query_handler));
}

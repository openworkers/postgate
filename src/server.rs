use actix_web::{HttpRequest, HttpResponse, web};
use uuid::Uuid;

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

fn extract_database_id(req: &HttpRequest) -> Result<Uuid, PostgateError> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .ok_or(PostgateError::MissingAuth)?
        .to_str()
        .map_err(|_| PostgateError::InvalidAuth)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(PostgateError::InvalidAuth)?;

    Uuid::parse_str(token).map_err(|_| PostgateError::InvalidAuth)
}

pub async fn query_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, PostgateError> {
    let database_id = extract_database_id(&req)?;

    // Load database config from store
    let db_config = state
        .store
        .get_database(database_id)
        .await
        .map_err(|_| PostgateError::DatabaseNotFound(database_id))?;

    // Parse and validate SQL
    let _parsed = parse_and_validate(&body.sql, &db_config.rules)?;

    // Execute query
    let response: QueryResponse = state
        .executor_pool
        .execute(
            database_id,
            &db_config.backend,
            &body,
            db_config.rules.max_rows,
            db_config.rules.timeout_seconds,
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

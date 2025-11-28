use actix_web::{HttpRequest, HttpResponse, web};
use uuid::Uuid;

use crate::auth::{JwtValidator, extract_database_id};
use crate::config::Config;
use crate::error::PostgateError;
use crate::executor::{ExecutorPool, QueryRequest, QueryResponse};
use crate::parser::parse_and_validate;
use crate::store::Store;

pub struct AppState {
    pub config: Config,
    pub executor_pool: ExecutorPool,
    pub store: Store,
    pub jwt_validator: JwtValidator,
}

impl AppState {
    pub fn new(config: Config, executor_pool: ExecutorPool, store: Store) -> Self {
        let jwt_validator = JwtValidator::new(&config.jwt_secret);
        Self {
            config,
            executor_pool,
            store,
            jwt_validator,
        }
    }
}

fn get_database_id(req: &HttpRequest, state: &AppState) -> Result<Uuid, PostgateError> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok());

    extract_database_id(auth_header, &state.jwt_validator).map_err(|e| match e {
        crate::auth::AuthError::MissingHeader => PostgateError::MissingAuth,
        _ => PostgateError::InvalidAuth,
    })
}

pub async fn query_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, PostgateError> {
    let database_id = get_database_id(&req, &state)?;

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

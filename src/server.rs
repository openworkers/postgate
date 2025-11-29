use actix_web::{HttpRequest, HttpResponse, web};
use uuid::Uuid;

use crate::auth::{TokenInfo, compute_token_hash, extract_token};
use crate::config::{Config, TokenPermission};
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

async fn get_token_info(req: &HttpRequest, state: &AppState) -> Result<TokenInfo, PostgateError> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok());

    let token = extract_token(auth_header).map_err(|e| match e {
        crate::auth::AuthError::MissingHeader => PostgateError::MissingAuth,
        _ => PostgateError::InvalidAuth,
    })?;

    let token_hash = compute_token_hash(&token);

    state
        .store
        .validate_token(&token_hash)
        .await
        .map_err(|_| PostgateError::InvalidAuth)
}

/// Admin database ID (postgate_admin with access to public schema)
const ADMIN_DATABASE_ID: Uuid = Uuid::from_u128(0);

/// Verify that the request is authenticated with an admin token
async fn require_admin(req: &HttpRequest, state: &AppState) -> Result<(), PostgateError> {
    let token_info = get_token_info(req, state).await?;

    if token_info.database_id != ADMIN_DATABASE_ID {
        return Err(PostgateError::Forbidden);
    }

    Ok(())
}

/// Default query timeout in seconds
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

pub async fn query_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, PostgateError> {
    let token_info = get_token_info(&req, &state).await?;

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

// ============ Token Management Endpoints ============

#[derive(serde::Deserialize)]
pub struct CreateTokenRequest {
    pub database_id: Uuid,
    #[serde(default = "default_token_name")]
    pub name: String,
    /// Permissions for this token. If not provided, uses default (DML only)
    pub permissions: Option<Vec<TokenPermission>>,
}

fn default_token_name() -> String {
    "default".to_string()
}

#[derive(serde::Serialize)]
pub struct CreateTokenResponse {
    pub id: Uuid,
    pub token: String,
    pub message: String,
}

/// Create a new token for a database
/// This endpoint requires admin authentication
pub async fn create_token_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<CreateTokenRequest>,
) -> Result<HttpResponse, PostgateError> {
    // Require admin token
    require_admin(&req, &state).await?;

    // Verify the database exists
    state
        .store
        .get_database(body.database_id)
        .await
        .map_err(|_| PostgateError::DatabaseNotFound(body.database_id))?;

    // Use provided permissions or default (DML only)
    let permissions = body
        .permissions
        .as_deref()
        .unwrap_or(TokenPermission::default_set());

    let (token_id, full_token) = state
        .store
        .create_token(body.database_id, &body.name, permissions)
        .await
        .map_err(|e| PostgateError::Internal(e.to_string()))?;

    Ok(HttpResponse::Ok().json(CreateTokenResponse {
        id: token_id,
        token: full_token,
        message: "Store this token securely. It will not be shown again.".to_string(),
    }))
}

#[derive(serde::Deserialize)]
pub struct DeleteTokenRequest {
    pub token_id: Uuid,
}

/// Delete a token
/// This endpoint requires admin authentication
pub async fn delete_token_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<DeleteTokenRequest>,
) -> Result<HttpResponse, PostgateError> {
    // Require admin token
    require_admin(&req, &state).await?;

    state
        .store
        .delete_token(body.token_id)
        .await
        .map_err(|e| PostgateError::Internal(e.to_string()))?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true
    })))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health_handler))
        .route("/query", web::post().to(query_handler))
        // Token management (should be protected in production)
        .route("/tokens", web::post().to(create_token_handler))
        .route("/tokens", web::delete().to(delete_token_handler));
}

use actix_web::{HttpRequest, HttpResponse, web};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::{Config, TenantConfig};
use crate::error::PostgateError;
use crate::executor::{Executor, QueryRequest, QueryResponse};
use crate::parser::parse_and_validate;

pub struct AppState {
    pub config: Config,
    pub executors: RwLock<HashMap<String, Arc<Executor>>>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            executors: RwLock::new(HashMap::new()),
        }
    }

    async fn get_executor(&self, tenant_id: &str) -> Result<Arc<Executor>, PostgateError> {
        // Check if executor already exists
        {
            let executors = self.executors.read().await;
            if let Some(executor) = executors.get(tenant_id) {
                return Ok(executor.clone());
            }
        }

        // Create new executor
        let tenant_config = self
            .config
            .tenants
            .get(tenant_id)
            .ok_or_else(|| PostgateError::TenantNotFound(tenant_id.to_string()))?;

        let executor = Arc::new(
            Executor::new(&tenant_config.connection_string)
                .await
                .map_err(PostgateError::Executor)?,
        );

        // Store executor
        {
            let mut executors = self.executors.write().await;
            executors.insert(tenant_id.to_string(), executor.clone());
        }

        Ok(executor)
    }

    fn get_tenant_config(&self, tenant_id: &str) -> Result<&TenantConfig, PostgateError> {
        self.config
            .tenants
            .get(tenant_id)
            .ok_or_else(|| PostgateError::TenantNotFound(tenant_id.to_string()))
    }
}

fn extract_tenant_id(req: &HttpRequest) -> Result<String, PostgateError> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .ok_or(PostgateError::MissingAuth)?
        .to_str()
        .map_err(|_| PostgateError::InvalidAuth)?;

    if let Some(token) = auth_header.strip_prefix("Bearer ") {
        Ok(token.to_string())
    } else {
        Err(PostgateError::InvalidAuth)
    }
}

pub async fn query_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, PostgateError> {
    let tenant_id = extract_tenant_id(&req)?;
    let tenant_config = state.get_tenant_config(&tenant_id)?;

    // Parse and validate SQL
    let _parsed = parse_and_validate(&body.sql, &tenant_config.rules)?;

    // Execute query
    let executor = state.get_executor(&tenant_id).await?;
    let response: QueryResponse = executor
        .execute(
            &body,
            tenant_config.rules.max_rows,
            tenant_config.rules.timeout_seconds,
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

pub fn create_app(
    config: Config,
) -> actix_web::App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    let state = web::Data::new(AppState::new(config));
    actix_web::App::new()
        .app_data(state)
        .configure(configure_routes)
}

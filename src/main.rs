use actix_web::{App, HttpServer, web};
use std::collections::HashMap;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

use postgate::config::{Config, QueryRules, ServerConfig, SqlOperation, TenantConfig};
use postgate::server::{AppState, health_handler, query_handler};

fn load_config() -> Config {
    // For now, load a simple config from environment
    // In production, this would be loaded from a file or external source

    let host = env::var("POSTGATE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("POSTGATE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let mut tenants = HashMap::new();

    // Load default tenant from DATABASE_URL if present
    if let Ok(database_url) = env::var("DATABASE_URL") {
        let rules = QueryRules {
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
            max_rows: env::var("POSTGATE_MAX_ROWS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            timeout_seconds: env::var("POSTGATE_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
        };

        tenants.insert(
            "default".to_string(),
            TenantConfig {
                connection_string: database_url,
                rules,
            },
        );
    }

    Config {
        server: ServerConfig { host, port },
        tenants,
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("postgate=info".parse().unwrap()),
        )
        .init();

    let config = load_config();
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);

    info!("Starting postgate server on {}", bind_addr);
    info!("Loaded {} tenant(s)", config.tenants.len());

    let state = web::Data::new(AppState::new(config));

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .route("/health", web::get().to(health_handler))
            .route("/query", web::post().to(query_handler))
    })
    .bind(&bind_addr)?
    .run()
    .await
}

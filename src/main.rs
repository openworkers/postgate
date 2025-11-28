use actix_web::{App, HttpServer, web};
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

use postgate::config::{Config, ServerConfig};
use postgate::executor::ExecutorPool;
use postgate::server::{AppState, configure_routes};
use postgate::store::Store;

fn load_config() -> Config {
    let host = env::var("POSTGATE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("POSTGATE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let database_url =
        env::var("DATABASE_URL").expect("DATABASE_URL environment variable is required");

    Config {
        server: ServerConfig { host, port },
        database_url,
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

    // Create executor pool (shared connection pool)
    let executor_pool = ExecutorPool::new(&config.database_url)
        .await
        .expect("Failed to create executor pool");

    // Run migrations
    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(executor_pool.shared_pool())
        .await
        .expect("Failed to run migrations");
    info!("Migrations completed");

    // Create store (uses the shared pool)
    let store = Store::new(executor_pool.shared_pool().clone());

    let state = web::Data::new(AppState::new(config, executor_pool, store));

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .configure(configure_routes)
    })
    .bind(&bind_addr)?
    .run()
    .await
}

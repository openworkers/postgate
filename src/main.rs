use actix_web::{App, HttpServer, web};
use clap::{Parser, Subcommand};
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use postgate::config::{Config, ServerConfig};
use postgate::executor::ExecutorPool;
use postgate::server::{AppState, configure_routes};
use postgate::store::Store;
use postgate::token::generate_token;

/// Secure HTTP proxy for PostgreSQL with SQL validation and multi-tenant support
#[derive(Parser)]
#[command(name = "postgate")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a token for a database
    GenToken {
        /// Database UUID
        database_id: String,

        /// Token name
        #[arg(default_value = "default")]
        name: String,

        /// Comma-separated permissions: SELECT,INSERT,UPDATE,DELETE,CREATE,ALTER,DROP
        #[arg(short, long, default_value = "SELECT,INSERT,UPDATE,DELETE")]
        permissions: String,
    },
}

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

async fn generate_token_command(
    database_id: &str,
    name: &str,
    permissions_str: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let database_url =
        env::var("DATABASE_URL").expect("DATABASE_URL environment variable is required");

    let pool = sqlx::PgPool::connect(&database_url).await?;

    // Parse database_id
    let db_id: Uuid = database_id
        .parse()
        .map_err(|_| format!("Invalid database ID: {}", database_id))?;

    // Generate token
    let (token, token_hash, token_prefix) = generate_token();

    // Parse permissions
    let permissions: Vec<&str> = permissions_str.split(',').map(|s| s.trim()).collect();

    // Validate permissions
    let valid_ops = [
        "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP",
    ];
    for perm in &permissions {
        if !valid_ops.contains(perm) {
            return Err(format!("Invalid permission: {}. Valid: {:?}", perm, valid_ops).into());
        }
    }

    // Insert into database
    sqlx::query(
        r#"
        INSERT INTO postgate_tokens (database_id, name, token_hash, token_prefix, allowed_operations)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (database_id, name) DO UPDATE SET
            token_hash = EXCLUDED.token_hash,
            token_prefix = EXCLUDED.token_prefix,
            allowed_operations = EXCLUDED.allowed_operations,
            created_at = NOW()
        "#,
    )
    .bind(db_id)
    .bind(name)
    .bind(&token_hash)
    .bind(&token_prefix)
    .bind(&permissions)
    .execute(&pool)
    .await?;

    println!("{}", token);

    Ok(())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::GenToken {
                database_id,
                name,
                permissions,
            } => {
                if let Err(e) = generate_token_command(&database_id, &name, &permissions).await {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return Ok(());
            }
        }
    }

    // Start server
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

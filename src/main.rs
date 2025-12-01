use actix_web::{App, HttpServer, web};
use clap::{Parser, Subcommand};
use log::info;
use std::env;
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
    /// Create a new tenant database
    CreateDb {
        /// Database name
        name: String,

        /// Maximum rows per query
        #[arg(short, long, default_value = "1000")]
        max_rows: i32,

        /// Use dedicated connection string instead of schema isolation
        #[arg(short, long)]
        dedicated: Option<String>,
    },

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

    let database_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            let host = std::env::var("POSTGRES_HOST").expect("POSTGRES_HOST must be set");
            let port = std::env::var("POSTGRES_PORT").expect("POSTGRES_PORT must be set");
            let user = std::env::var("POSTGRES_USER").expect("POSTGRES_USER must be set");
            let password =
                std::env::var("POSTGRES_PASSWORD").expect("POSTGRES_PASSWORD must be set");
            let database = std::env::var("POSTGRES_DB").expect("POSTGRES_DB must be set");

            log::debug!("DATABASE_URL not set, using POSTGRES_* env vars");

            format!("postgres://{user}:{password}@{host}:{port}/{database}")
        }
    };

    Config {
        server: ServerConfig { host, port },
        database_url,
    }
}

async fn create_db_command(
    name: &str,
    max_rows: i32,
    dedicated: Option<&str>,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect(&config.database_url).await?;

    match dedicated {
        Some(connection_string) => {
            // Dedicated database
            let row: (Uuid,) = sqlx::query_as(
                r#"
                INSERT INTO postgate_databases (name, backend_type, connection_string, max_rows)
                VALUES ($1, 'dedicated', $2, $3)
                RETURNING id
                "#,
            )
            .bind(name)
            .bind(connection_string)
            .bind(max_rows)
            .fetch_one(&pool)
            .await?;

            println!("{}", row.0);
        }
        None => {
            // Schema-based isolation (default)
            let row: (Uuid, String) =
                sqlx::query_as("SELECT id, schema_name FROM create_tenant_database($1, $2)")
                    .bind(name)
                    .bind(max_rows)
                    .fetch_one(&pool)
                    .await?;

            println!("{}", row.0);
            eprintln!("Schema: {}", row.1);
        }
    }

    Ok(())
}

async fn generate_token_command(
    database_id: &str,
    name: &str,
    permissions_str: &str,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect(&config.database_url).await?;

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
    env_logger::init();

    log::debug!("start main");

    let cli = Cli::parse();

    let config = load_config();

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::CreateDb {
                name,
                max_rows,
                dedicated,
            } => {
                if let Err(e) =
                    create_db_command(&name, max_rows, dedicated.as_deref(), &config).await
                {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return Ok(());
            }
            Commands::GenToken {
                database_id,
                name,
                permissions,
            } => {
                if let Err(e) =
                    generate_token_command(&database_id, &name, &permissions, &config).await
                {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                return Ok(());
            }
        }
    }

    // Start server
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

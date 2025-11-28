use actix_web::test;
use jsonwebtoken::{EncodingKey, Header, encode};
use postgate::auth::Claims;
use postgate::config::{Config, DatabaseBackend, QueryRules, ServerConfig, SqlOperation};
use postgate::executor::ExecutorPool;
use postgate::server::{AppState, configure_routes};
use postgate::store::{Store, generate_schema_name};
use serde_json::json;
use std::collections::HashSet;
use uuid::Uuid;

const TEST_JWT_SECRET: &str = "test_secret_for_integration_tests";

fn create_jwt(database_id: &Uuid) -> String {
    let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: database_id.to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

async fn setup_test_app() -> (
    impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
    >,
    String, // JWT token instead of Uuid
) {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgate:password@localhost/postgate_test".to_string());

    let executor_pool = ExecutorPool::new(&database_url)
        .await
        .expect("Failed to create pool");

    let store = Store::new(executor_pool.shared_pool().clone());

    // Create a test database entry
    let user_id = Uuid::new_v4();
    let db_name = format!("test_{}", &Uuid::new_v4().to_string()[..8]);
    let schema_name = generate_schema_name(user_id, &db_name);

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
        denied_tables: HashSet::new(),
        max_rows: 1000,
        timeout_seconds: 30,
    };

    let db_config = store
        .create_database(
            user_id,
            &db_name,
            &DatabaseBackend::Schema {
                schema_name: schema_name.clone(),
            },
            &rules,
        )
        .await
        .expect("Failed to create test database");

    // Create a test table in the schema
    sqlx::query(&format!(
        "CREATE TABLE \"{}\".users (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
        schema_name
    ))
    .execute(executor_pool.shared_pool())
    .await
    .expect("Failed to create test table");

    // Insert test data
    sqlx::query(&format!(
        "INSERT INTO \"{}\".users (name) VALUES ('Alice'), ('Bob')",
        schema_name
    ))
    .execute(executor_pool.shared_pool())
    .await
    .expect("Failed to insert test data");

    let config = Config {
        server: ServerConfig::default(),
        database_url,
        jwt_secret: TEST_JWT_SECRET.to_string(),
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

    let token = create_jwt(&db_config.id);
    (app, token)
}

#[actix_web::test]
async fn test_health_endpoint() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgate:password@localhost/postgate_test".to_string());

    let executor_pool = ExecutorPool::new(&database_url)
        .await
        .expect("Failed to create pool");

    let store = Store::new(executor_pool.shared_pool().clone());

    let config = Config {
        server: ServerConfig::default(),
        database_url,
        jwt_secret: TEST_JWT_SECRET.to_string(),
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

    let req = test::TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;

    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

#[actix_web::test]
async fn test_query_missing_auth() {
    let (app, _token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "UNAUTHORIZED");
}

#[actix_web::test]
async fn test_query_invalid_token() {
    let (app, _token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer invalid-jwt-token"))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
}

#[actix_web::test]
async fn test_query_unknown_database() {
    let (app, _token) = setup_test_app().await;

    // Create a valid JWT but for a non-existent database
    let fake_id = Uuid::new_v4();
    let fake_token = create_jwt(&fake_id);

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", fake_token)))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "DATABASE_NOT_FOUND");
}

#[actix_web::test]
async fn test_query_select() {
    let (app, token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({"sql": "SELECT * FROM users", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["rows"].is_array());
    assert_eq!(body["row_count"], 2);
}

#[actix_web::test]
async fn test_query_with_params() {
    let (app, token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({"sql": "SELECT * FROM users WHERE name = $1", "params": ["Alice"]}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["row_count"], 1);
}

#[actix_web::test]
async fn test_query_insert() {
    let (app, token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({"sql": "INSERT INTO users (name) VALUES ($1) RETURNING *", "params": ["Charlie"]}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["row_count"], 1);
}

#[actix_web::test]
async fn test_query_invalid_sql() {
    let (app, token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({"sql": "SELEKT * FORM users", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_query_multiple_statements_rejected() {
    let (app, token) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({"sql": "SELECT 1; SELECT 2", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_query_expired_token() {
    let (app, _token) = setup_test_app().await;

    // Create an expired JWT
    let db_id = Uuid::new_v4();
    let exp = (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: db_id.to_string(),
        exp,
    };
    let expired_token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .unwrap();

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", expired_token)))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
}

// Helper to setup an admin app (with access to public schema)
async fn setup_admin_app() -> (
    impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
    >,
    String, // Admin JWT token
) {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgate:password@localhost/postgate_test".to_string());

    let executor_pool = ExecutorPool::new(&database_url)
        .await
        .expect("Failed to create pool");

    // Run migrations to ensure functions exist (ignore errors if already run)
    let _ = sqlx::migrate!("./migrations")
        .run(executor_pool.shared_pool())
        .await;

    let store = Store::new(executor_pool.shared_pool().clone());

    // Create admin database entry with access to public schema
    let admin_id = Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap();
    let admin_user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();

    // Delete if exists (from previous test runs)
    let _ = sqlx::query("DELETE FROM postgate_databases WHERE id = $1")
        .bind(admin_id)
        .execute(executor_pool.shared_pool())
        .await;

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
        denied_tables: HashSet::new(),
        max_rows: 1000,
        timeout_seconds: 30,
    };

    // Insert admin database with public schema access
    sqlx::query(
        r#"INSERT INTO postgate_databases (id, user_id, name, backend_type, schema_name, rules)
           VALUES ($1, $2, 'admin', 'schema', 'public', $3)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(admin_id)
    .bind(admin_user_id)
    .bind(serde_json::to_value(&rules).unwrap())
    .execute(executor_pool.shared_pool())
    .await
    .expect("Failed to create admin database");

    let config = Config {
        server: ServerConfig::default(),
        database_url,
        jwt_secret: TEST_JWT_SECRET.to_string(),
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

    let token = create_jwt(&admin_id);
    (app, token)
}

#[actix_web::test]
async fn test_admin_can_create_tenant() {
    let (app, admin_token) = setup_admin_app().await;

    let user_id = Uuid::new_v4();
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1::uuid, $2)",
            "params": [user_id.to_string(), "test_db"]
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    let status = resp.status();
    let body: serde_json::Value = test::read_body_json(resp).await;

    if !status.is_success() {
        panic!("Admin create tenant failed: {} - {:?}", status, body);
    }

    assert_eq!(body["row_count"], 1);
    assert!(body["rows"][0]["id"].is_string());
    assert!(body["rows"][0]["schema_name"].is_string());
}

#[actix_web::test]
async fn test_admin_can_list_tenants() {
    let (app, admin_token) = setup_admin_app().await;

    let user_id = Uuid::new_v4();
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM list_tenant_databases($1::uuid)",
            "params": [user_id.to_string()]
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
}

#[actix_web::test]
async fn test_admin_can_delete_tenant() {
    let (app, admin_token) = setup_admin_app().await;

    // First create a tenant
    let user_id = Uuid::new_v4();
    let create_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1::uuid, $2)",
            "params": [user_id.to_string(), "to_delete"]
        }))
        .to_request();

    let resp = test::call_service(&app, create_req).await;
    let body: serde_json::Value = test::read_body_json(resp).await;
    let tenant_id = body["rows"][0]["id"].as_str().unwrap();

    // Now delete it
    let delete_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT delete_tenant_database($1::uuid)",
            "params": [tenant_id]
        }))
        .to_request();

    let resp = test::call_service(&app, delete_req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["rows"][0]["delete_tenant_database"], true);
}

#[actix_web::test]
async fn test_tenant_cannot_call_admin_functions() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to call create_tenant_database from a regular tenant
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1, $2)",
            "params": [Uuid::new_v4().to_string(), "hacked_db"]
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;

    // Should fail because the function is in public schema,
    // but tenant's search_path is their own schema
    assert!(!resp.status().is_success());
}

#[actix_web::test]
async fn test_tenant_cannot_access_postgate_databases_table() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to directly query postgate_databases
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT * FROM postgate_databases",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;

    // Should fail - table is in public schema, not accessible from tenant schema
    assert!(!resp.status().is_success());
}

// Security tests - ensure tenant isolation

#[actix_web::test]
async fn test_tenant_cannot_access_other_schema_qualified() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to access public schema with qualified name
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT * FROM public.postgate_databases",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(
        !resp.status().is_success(),
        "Should not access public.postgate_databases"
    );
}

#[actix_web::test]
async fn test_tenant_cannot_list_schemas() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to list all schemas via information_schema (qualified name - should be blocked)
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT schema_name FROM information_schema.schemata",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    // Should fail at parse time - qualified table names are blocked
    assert_eq!(
        resp.status(),
        400,
        "Should block information_schema.schemata"
    );

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_tenant_cannot_list_all_tables() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to list all tables via information_schema (qualified name - should be blocked)
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT table_schema, table_name FROM information_schema.tables",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    // Should fail at parse time - qualified table names are blocked
    assert_eq!(resp.status(), 400, "Should block information_schema.tables");

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_tenant_cannot_use_pg_catalog() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to query pg_catalog.pg_namespace (qualified name - should be blocked)
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT nspname FROM pg_catalog.pg_namespace",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    // Should fail at parse time - qualified table names are blocked
    assert_eq!(resp.status(), 400, "Should block pg_catalog.pg_namespace");

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_tenant_cannot_access_pg_tables_directly() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to query pg_tables directly (system table - should be blocked)
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT * FROM pg_tables",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    // Should fail at parse time - pg_* tables are blocked
    assert_eq!(resp.status(), 400, "Should block pg_tables");

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_tenant_cannot_call_admin_function_qualified() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to call admin function with schema-qualified name
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT * FROM public.create_tenant_database($1::uuid, $2)",
            "params": [Uuid::new_v4().to_string(), "hacked"]
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(
        !resp.status().is_success(),
        "Should not call public.create_tenant_database"
    );
}

#[actix_web::test]
async fn test_tenant_cannot_set_search_path() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to change search_path via SQL
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SET search_path TO public",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    // Should fail - SET is not SELECT/INSERT/UPDATE/DELETE
    assert!(!resp.status().is_success(), "Should not allow SET command");
}

#[actix_web::test]
async fn test_tenant_cannot_create_schema() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to create a new schema
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "CREATE SCHEMA hacked_schema",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(
        !resp.status().is_success(),
        "Should not allow CREATE SCHEMA"
    );
}

#[actix_web::test]
async fn test_tenant_cannot_drop_schema() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to drop a schema
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "DROP SCHEMA public CASCADE",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(!resp.status().is_success(), "Should not allow DROP SCHEMA");
}

// Test dedicated backend mode (separate connection pool)
#[actix_web::test]
async fn test_dedicated_backend() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgate:password@localhost/postgate_test".to_string());

    let executor_pool = ExecutorPool::new(&database_url)
        .await
        .expect("Failed to create pool");

    let store = Store::new(executor_pool.shared_pool().clone());

    // Create a dedicated database entry (using the same connection string for test)
    let user_id = Uuid::new_v4();
    let dedicated_id = Uuid::new_v4();

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
        denied_tables: HashSet::new(),
        max_rows: 1000,
        timeout_seconds: 30,
    };

    // Insert dedicated database entry
    sqlx::query(
        r#"INSERT INTO postgate_databases (id, user_id, name, backend_type, connection_string, rules)
           VALUES ($1, $2, 'dedicated_test', 'dedicated', $3, $4)"#,
    )
    .bind(dedicated_id)
    .bind(user_id)
    .bind(&database_url) // Same DB but tests dedicated code path
    .bind(serde_json::to_value(&rules).unwrap())
    .execute(executor_pool.shared_pool())
    .await
    .expect("Failed to create dedicated database entry");

    let config = Config {
        server: ServerConfig::default(),
        database_url: database_url.clone(),
        jwt_secret: TEST_JWT_SECRET.to_string(),
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

    let token = create_jwt(&dedicated_id);

    // Query should work - dedicated mode doesn't use search_path, queries public schema directly
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({"sql": "SELECT COUNT(*) as cnt FROM postgate_databases", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    let status = resp.status();
    let body: serde_json::Value = test::read_body_json(resp).await;

    if !status.is_success() {
        panic!("Dedicated query failed: {} - {:?}", status, body);
    }

    assert_eq!(body["row_count"], 1);
    // Should have at least 1 row (the dedicated entry we just created)
    assert!(body["rows"][0]["cnt"].as_i64().unwrap() >= 1);
}

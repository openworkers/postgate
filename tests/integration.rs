use actix_web::test;
use postgate::config::{Config, DatabaseBackend, ServerConfig, TokenPermission};
use postgate::executor::ExecutorPool;
use postgate::server::{AppState, configure_routes};
use postgate::store::{Store, generate_schema_name};
use postgate::token::generate_token;
use serde_json::json;
use uuid::Uuid;

async fn setup_test_app() -> (
    impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
    >,
    String, // API token
) {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgate:password@localhost/postgate_test".to_string());

    let executor_pool = ExecutorPool::new(&database_url)
        .await
        .expect("Failed to create pool");

    // Run migrations
    let _ = sqlx::migrate!("./migrations")
        .run(executor_pool.shared_pool())
        .await;

    let store = Store::new(executor_pool.shared_pool().clone());

    // Create a test database entry
    let db_name = format!("test_{}", &Uuid::new_v4().to_string()[..8]);
    let schema_name = generate_schema_name(&db_name);

    let db_config = store
        .create_database(
            &db_name,
            &DatabaseBackend::Schema {
                schema_name: schema_name.clone(),
            },
            1000, // max_rows
        )
        .await
        .expect("Failed to create test database");

    // Create a token for the database with tenant permissions (DML + DDL)
    let (_, token) = store
        .create_token(db_config.id, "test_token", TokenPermission::tenant_set())
        .await
        .expect("Failed to create token");

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
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

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
        .insert_header(("Authorization", "Bearer invalid-token"))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
}

#[actix_web::test]
async fn test_query_unknown_token() {
    let (app, _token) = setup_test_app().await;

    // Generate a valid format token that doesn't exist in DB
    let (fake_token, _, _) = generate_token();

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", fake_token)))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
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

// Helper to setup an admin app (with access to public schema)
async fn setup_admin_app() -> (
    impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
    >,
    String, // Admin token
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

    // Delete if exists (from previous test runs)
    let _ = sqlx::query("DELETE FROM postgate_tokens WHERE database_id = $1")
        .bind(admin_id)
        .execute(executor_pool.shared_pool())
        .await;

    let _ = sqlx::query("DELETE FROM postgate_databases WHERE id = $1")
        .bind(admin_id)
        .execute(executor_pool.shared_pool())
        .await;

    // Insert admin database with public schema access
    sqlx::query(
        r#"INSERT INTO postgate_databases (id, name, backend_type, schema_name, max_rows)
           VALUES ($1, 'admin', 'schema', 'public', 1000)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(admin_id)
    .execute(executor_pool.shared_pool())
    .await
    .expect("Failed to create admin database");

    // Create token for admin database with default permissions (DML only)
    let (_, admin_token) = store
        .create_token(admin_id, "admin_token", TokenPermission::default_set())
        .await
        .expect("Failed to create admin token");

    let config = Config {
        server: ServerConfig::default(),
        database_url,
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

    (app, admin_token)
}

#[actix_web::test]
async fn test_admin_can_create_tenant() {
    let (app, admin_token) = setup_admin_app().await;

    let db_name = format!("test_db_{}", &Uuid::new_v4().to_string()[..8]);
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1)",
            "params": [db_name]
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
async fn test_admin_can_list_databases() {
    let (app, admin_token) = setup_admin_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT id, name, backend_type, schema_name, max_rows FROM postgate_databases",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["rows"].is_array());
}

#[actix_web::test]
async fn test_admin_can_delete_tenant() {
    let (app, admin_token) = setup_admin_app().await;

    // First create a tenant
    let db_name = format!("to_delete_{}", &Uuid::new_v4().to_string()[..8]);
    let create_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1)",
            "params": [db_name]
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
async fn test_pg_class_access() {
    let (app, tenant_token) = setup_test_app().await;

    // Try to access pg_class (list all tables)
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", tenant_token)))
        .set_json(json!({
            "sql": "SELECT relname FROM pg_class WHERE relkind = 'r'",
            "params": []
        }))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400, "Should block pg_class access");
}

// Test token management via SQL (using PL/pgSQL functions)

#[actix_web::test]
async fn test_create_token_via_sql() {
    let (app, admin_token) = setup_admin_app().await;

    // First create a tenant database
    let db_name = format!("token_test_{}", &Uuid::new_v4().to_string()[..8]);
    let create_db_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1)",
            "params": [db_name]
        }))
        .to_request();

    let resp = test::call_service(&app, create_db_req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    let database_id = body["rows"][0]["id"].as_str().unwrap();

    // Create a token via SQL function
    let create_token_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_token($1::uuid, $2, $3::text[])",
            "params": [database_id, "my_token", ["SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP"]]
        }))
        .to_request();

    let resp = test::call_service(&app, create_token_req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["rows"][0]["id"].is_string());
    assert!(
        body["rows"][0]["token"]
            .as_str()
            .unwrap()
            .starts_with("pg_")
    );
}

#[actix_web::test]
async fn test_delete_token_via_sql() {
    let (app, admin_token) = setup_admin_app().await;

    // Create a tenant database
    let db_name = format!("delete_token_test_{}", &Uuid::new_v4().to_string()[..8]);
    let create_db_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_database($1)",
            "params": [db_name]
        }))
        .to_request();

    let resp = test::call_service(&app, create_db_req).await;
    let body: serde_json::Value = test::read_body_json(resp).await;
    let database_id = body["rows"][0]["id"].as_str().unwrap();

    // Create a token
    let create_token_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT * FROM create_tenant_token($1::uuid)",
            "params": [database_id]
        }))
        .to_request();

    let resp = test::call_service(&app, create_token_req).await;
    let body: serde_json::Value = test::read_body_json(resp).await;
    let token_id = body["rows"][0]["id"].as_str().unwrap();

    // Delete the token
    let delete_token_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", admin_token)))
        .set_json(json!({
            "sql": "SELECT delete_tenant_token($1::uuid)",
            "params": [token_id]
        }))
        .to_request();

    let resp = test::call_service(&app, delete_token_req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["rows"][0]["delete_tenant_token"], true);
}

// Regression test for intermittent "invalid byte sequence for encoding UTF8: 0x00"
// triggered when binding numeric JSON params to INT4/REAL columns with explicit casts.
// Without the fix, sqlx encodes JSON numbers as i64/f64 (8-byte big-endian binary).
// The high zero bytes of small ints occasionally get re-decoded as UTF-8 by Postgres,
// producing the 0x00 byte error on the param's portal. We loop many iterations with
// values that have leading zero bytes in big-endian (small ints, zero, null) to maximize
// the chance of hitting the bug.
#[actix_web::test]
async fn test_numeric_params_with_explicit_casts() {
    let (app, token) = setup_test_app().await;

    // Create a table with the same shape as a real-world failing case
    let create_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({
            "sql": "CREATE TABLE numerics (id text PRIMARY KEY, val_int int, val_real real, val_int_nullable int, val_real_nullable real)",
            "params": []
        }))
        .to_request();
    let resp = test::call_service(&app, create_req).await;
    assert!(resp.status().is_success(), "CREATE TABLE failed");

    // Seed one row
    let insert_req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", token)))
        .set_json(json!({
            "sql": "INSERT INTO numerics (id, val_int, val_real) VALUES ($1::text, $2::int, $3::real)",
            "params": ["row1", 0, 0.0]
        }))
        .to_request();
    let resp = test::call_service(&app, insert_req).await;
    assert!(resp.status().is_success(), "Initial INSERT failed");

    // Many UPDATEs with mixed numeric params (small ints, zeros, nulls) — these are the
    // values whose binary encoding contains leading 0x00 bytes that trigger the driver bug.
    let mut errors: Vec<String> = Vec::new();
    let test_values: Vec<(serde_json::Value, serde_json::Value, serde_json::Value, serde_json::Value)> = vec![
        (json!(72), json!(9.5), json!(null), json!(null)),
        (json!(0), json!(0.0), json!(0), json!(0.0)),
        (json!(1), json!(0.1), json!(null), json!(null)),
        (json!(500), json!(12.34), json!(72), json!(9.5)),
        (json!(null), json!(null), json!(null), json!(null)),
    ];

    for iter in 0..50 {
        let (vi, vr, vin, vrn) = &test_values[iter % test_values.len()];
        let req = test::TestRequest::post()
            .uri("/query")
            .insert_header(("Authorization", format!("Bearer {}", token)))
            .set_json(json!({
                "sql": "UPDATE numerics SET val_int = $1::int, val_real = $2::real, val_int_nullable = $3::int, val_real_nullable = $4::real WHERE id = $5::text",
                "params": [vi, vr, vin, vrn, "row1"]
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: serde_json::Value = test::read_body_json(resp).await;
            errors.push(format!("iter {} status {}: {}", iter, status, body));
        }
    }

    assert!(
        errors.is_empty(),
        "{} of 50 numeric UPDATEs failed:\n{}",
        errors.len(),
        errors.join("\n")
    );
}

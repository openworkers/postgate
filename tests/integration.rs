use actix_web::test;
use postgate::config::{Config, DatabaseBackend, QueryRules, ServerConfig, SqlOperation};
use postgate::executor::ExecutorPool;
use postgate::server::{AppState, configure_routes};
use postgate::store::{Store, generate_schema_name};
use serde_json::json;
use std::collections::HashSet;
use uuid::Uuid;

async fn setup_test_app() -> (
    impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
    >,
    Uuid,
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
    };

    let state = actix_web::web::Data::new(AppState::new(config, executor_pool, store));

    let app = test::init_service(
        actix_web::App::new()
            .app_data(state)
            .configure(configure_routes),
    )
    .await;

    (app, db_config.id)
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
    let (app, _db_id) = setup_test_app().await;

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
async fn test_query_invalid_uuid() {
    let (app, _db_id) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer not-a-uuid"))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 401);
}

#[actix_web::test]
async fn test_query_unknown_database() {
    let (app, _db_id) = setup_test_app().await;

    let fake_id = Uuid::new_v4();
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", fake_id)))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "DATABASE_NOT_FOUND");
}

#[actix_web::test]
async fn test_query_select() {
    let (app, db_id) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", db_id)))
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
    let (app, db_id) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", db_id)))
        .set_json(json!({"sql": "SELECT * FROM users WHERE name = $1", "params": ["Alice"]}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["row_count"], 1);
}

#[actix_web::test]
async fn test_query_insert() {
    let (app, db_id) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", db_id)))
        .set_json(json!({"sql": "INSERT INTO users (name) VALUES ($1) RETURNING *", "params": ["Charlie"]}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["row_count"], 1);
}

#[actix_web::test]
async fn test_query_invalid_sql() {
    let (app, db_id) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", db_id)))
        .set_json(json!({"sql": "SELEKT * FORM users", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_query_multiple_statements_rejected() {
    let (app, db_id) = setup_test_app().await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", format!("Bearer {}", db_id)))
        .set_json(json!({"sql": "SELECT 1; SELECT 2", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

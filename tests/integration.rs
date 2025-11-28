use actix_web::test;
use postgate::config::{Config, QueryRules, ServerConfig, SqlOperation, TenantConfig};
use postgate::server::create_app;
use serde_json::json;
use std::collections::{HashMap, HashSet};

fn test_config() -> Config {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgate:password@localhost/postgate_test".to_string());

    let mut tenants = HashMap::new();
    tenants.insert(
        "default".to_string(),
        TenantConfig {
            connection_string: database_url.clone(),
            rules: QueryRules {
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
            },
        },
    );

    // Add a restricted tenant for testing rules
    tenants.insert(
        "restricted".to_string(),
        TenantConfig {
            connection_string: database_url,
            rules: QueryRules {
                allowed_operations: [SqlOperation::Select].into_iter().collect(),
                allowed_tables: Some(["users".to_string()].into_iter().collect()),
                denied_tables: HashSet::new(),
                max_rows: 10,
                timeout_seconds: 5,
            },
        },
    );

    Config {
        server: ServerConfig::default(),
        tenants,
    }
}

#[actix_web::test]
async fn test_health_endpoint() {
    let app = test::init_service(create_app(test_config())).await;

    let req = test::TestRequest::get().uri("/health").to_request();
    let resp = test::call_service(&app, req).await;

    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

#[actix_web::test]
async fn test_query_missing_auth() {
    let app = test::init_service(create_app(test_config())).await;

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
async fn test_query_unknown_tenant() {
    let app = test::init_service(create_app(test_config())).await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer unknown"))
        .set_json(json!({"sql": "SELECT 1", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "TENANT_NOT_FOUND");
}

#[actix_web::test]
async fn test_query_select() {
    let app = test::init_service(create_app(test_config())).await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer default"))
        .set_json(json!({"sql": "SELECT * FROM users", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["rows"].is_array());
    assert!(body["row_count"].is_number());
}

#[actix_web::test]
async fn test_query_with_params() {
    let app = test::init_service(create_app(test_config())).await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer default"))
        .set_json(json!({"sql": "SELECT * FROM users WHERE name = $1", "params": ["Alice"]}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["row_count"].as_u64().unwrap() <= 1);
}

#[actix_web::test]
async fn test_query_operation_not_allowed() {
    let app = test::init_service(create_app(test_config())).await;

    // restricted tenant only allows SELECT
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer restricted"))
        .set_json(json!({"sql": "DELETE FROM users WHERE id = $1", "params": ["00000000-0000-0000-0000-000000000000"]}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_query_table_not_allowed() {
    let app = test::init_service(create_app(test_config())).await;

    // restricted tenant only allows 'users' table
    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer restricted"))
        .set_json(json!({"sql": "SELECT * FROM secrets", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_query_multiple_statements_rejected() {
    let app = test::init_service(create_app(test_config())).await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer default"))
        .set_json(json!({"sql": "SELECT 1; SELECT 2", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

#[actix_web::test]
async fn test_query_invalid_sql() {
    let app = test::init_service(create_app(test_config())).await;

    let req = test::TestRequest::post()
        .uri("/query")
        .insert_header(("Authorization", "Bearer default"))
        .set_json(json!({"sql": "SELEKT * FORM users", "params": []}))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["code"], "PARSE_ERROR");
}

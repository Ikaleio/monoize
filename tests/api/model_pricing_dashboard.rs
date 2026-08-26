use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use monoize::app::{RuntimeConfig, build_app, load_state_with_runtime};
use monoize::users::UserRole;
use serde_json::{Value, json};
use tower::ServiceExt;

struct TestContext {
    router: axum::Router,
    auth_header: String,
}

async fn setup() -> TestContext {
    let state = load_state_with_runtime(RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: monoize::node_config::NodeSettings::primary_default(),
    })
    .await
    .expect("state loads");
    let admin = state
        .user_store
        .create_user("admin_model_prices", "password", UserRole::Admin, None)
        .await
        .expect("admin created");
    let session = state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("session created");

    TestContext {
        router: build_app(state),
        auth_header: format!("Bearer {}", session.token),
    }
}

async fn json_request(
    ctx: &TestContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(AUTHORIZATION, ctx.auth_header.clone());
    let body = if let Some(body) = body {
        builder = builder.header(CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = ctx
        .router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    (status, value)
}

#[tokio::test]
async fn model_price_crud_validates_and_removes_legacy_endpoints() {
    let ctx = setup().await;
    let path = "/api/dashboard/model-prices/gpt-test";

    let (status, created) = json_request(
        &ctx,
        Method::PUT,
        path,
        Some(json!({
            "billing_mode": "per_token",
            "input_usd_per_1m": "1.25",
            "output_usd_per_1m": "5",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["model_id"], json!("gpt-test"));
    assert_eq!(created["source"], json!("manual"));
    assert_eq!(created["input_usd_per_1m"], json!("1.25"));
    assert_eq!(
        created["locked_fields"],
        json!(["input_usd_per_1m", "output_usd_per_1m"])
    );

    let (status, invalid) = json_request(
        &ctx,
        Method::PUT,
        "/api/dashboard/model-prices/invalid",
        Some(json!({ "input_usd_per_1m": "1e-3" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid["error"]["code"], json!("invalid_request"));

    let (status, rows) = json_request(&ctx, Method::GET, "/api/dashboard/model-prices", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows.as_array().map(Vec::len), Some(1));

    for (method, legacy_path) in [
        (Method::POST, "/api/dashboard/billing-rates/sync/catalog"),
        (Method::PUT, "/api/dashboard/pricing-profile-patterns"),
        (
            Method::POST,
            "/api/dashboard/model-metadata/sync/models-dev",
        ),
    ] {
        let (status, _) = json_request(&ctx, method, legacy_path, Some(json!({}))).await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
            "{legacy_path} returned {status}"
        );
    }

    let (status, deleted) = json_request(&ctx, Method::DELETE, path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["success"], json!(true));
}

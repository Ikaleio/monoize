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
    })
    .await
    .expect("state loads");
    let admin = state
        .user_store
        .create_user("admin_billing_rates", "password", UserRole::Admin, &[])
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
    let resp = ctx
        .router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    (status, value)
}

#[tokio::test]
async fn billing_rates_crud_catalog_sync_and_profile_patterns_api() {
    let ctx = setup().await;
    let rate_id = "openai:gpt-image-2:text:input";
    let manual_rate = json!({
        "source": "manual",
        "pricing_profile": "openai",
        "model_pattern": "gpt-image-2",
        "provider_type": "openai_image",
        "rate_kind": "token",
        "usage_class": "input_uncached",
        "unit": "token",
        "unit_price_nano_usd": "999",
        "modality": "text",
        "priority": 999,
        "enabled": true,
        "match_json": {},
        "raw_json": {}
    });

    let (status, created) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/billing-rates/{rate_id}"),
        Some(manual_rate),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["id"], json!(rate_id));
    assert_eq!(created["unit_price_nano_usd"], json!("999"));

    let (status, sync_result) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-rates/sync/catalog",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sync_result["success"], json!(true));
    assert!(
        sync_result["skipped"].as_u64().unwrap_or_default() >= 1,
        "manual override with catalog id must be skipped"
    );

    let (status, rates) =
        json_request(&ctx, Method::GET, "/api/dashboard/billing-rates", None).await;
    assert_eq!(status, StatusCode::OK);
    let preserved = rates
        .as_array()
        .expect("rates array")
        .iter()
        .find(|rate| rate["id"] == rate_id)
        .expect("manual catalog override remains");
    assert_eq!(preserved["source"], json!("manual"));
    assert_eq!(preserved["unit_price_nano_usd"], json!("999"));

    let catalog_rate_id = "openai:gpt-image-2:image:output";
    let (status, updated_catalog) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/billing-rates/{catalog_rate_id}"),
        Some(json!({ "unit_price_nano_usd": "12345" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated_catalog["source"], json!("manual"));
    assert_eq!(updated_catalog["unit_price_nano_usd"], json!("12345"));

    let (status, resync_result) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-rates/sync/catalog",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        resync_result["skipped"].as_u64().unwrap_or_default() >= 2,
        "manual edits of catalog rows must survive later syncs"
    );
    let (status, rates_after_resync) =
        json_request(&ctx, Method::GET, "/api/dashboard/billing-rates", None).await;
    assert_eq!(status, StatusCode::OK);
    let preserved_catalog_edit = rates_after_resync
        .as_array()
        .expect("rates array")
        .iter()
        .find(|rate| rate["id"] == catalog_rate_id)
        .expect("manual edit of catalog row remains");
    assert_eq!(preserved_catalog_edit["source"], json!("manual"));
    assert_eq!(
        preserved_catalog_edit["unit_price_nano_usd"],
        json!("12345")
    );

    let patterns = json!({
        "patterns": [
            { "pattern": "gpt-image-*", "pricing_profile": "openai-image" },
            { "pattern": "*", "pricing_profile": "default" }
        ]
    });
    let (status, updated) = json_request(
        &ctx,
        Method::PUT,
        "/api/dashboard/pricing-profile-patterns",
        Some(patterns),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated["patterns"][0]["pricing_profile"],
        json!("openai-image")
    );

    let (status, read_back) = json_request(
        &ctx,
        Method::GET,
        "/api/dashboard/pricing-profile-patterns",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read_back["patterns"][0]["pattern"], json!("gpt-image-*"));
    assert_eq!(
        read_back["patterns"][1]["pricing_profile"],
        json!("default")
    );

    let (status, deleted) = json_request(
        &ctx,
        Method::DELETE,
        &format!("/api/dashboard/billing-rates/{rate_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["success"], json!(true));
}

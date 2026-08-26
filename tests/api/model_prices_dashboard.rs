//! HTTP integration tests for the model pricing dashboard APIs
//! (`model-pricing.spec.md` §10: MP-A1..MP-A5, MP-Y17/MP-Y18).

use super::*;
use axum::body::Body as MpBody;
use axum::http::Method as MpMethod;
use axum::http::Request as MpRequest;
use axum::http::StatusCode as MpStatusCode;
use axum::http::header::{AUTHORIZATION as MP_AUTHORIZATION, CONTENT_TYPE as MP_CONTENT_TYPE};

async fn admin_header(ctx: &TestContext, username: &str) -> String {
    let admin = ctx
        .state
        .user_store
        .create_user(username, "password", monoize::users::UserRole::Admin, None)
        .await
        .expect("admin created");
    let session = ctx
        .state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("session created");
    format!("Bearer {}", session.token)
}

async fn json_call(
    ctx: &TestContext,
    method: MpMethod,
    path: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (MpStatusCode, Value) {
    let mut builder = MpRequest::builder().method(method).uri(path);
    if let Some(auth) = auth {
        builder = builder.header(MP_AUTHORIZATION, auth);
    }
    let body = if let Some(body) = body {
        builder = builder.header(MP_CONTENT_TYPE, "application/json");
        MpBody::from(body.to_string())
    } else {
        MpBody::empty()
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
async fn model_price_crud_lifecycle() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_crud").await;

    // MP-A1: empty list initially.
    let (status, body) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    assert_eq!(body, json!([]));

    // MP-A2: upsert creates a manual per-token row and locks edited fields
    // (MP-Y17).
    let (status, created) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/gpt-4o",
        Some(&admin),
        Some(json!({
            "billing_mode": "per_token",
            "input_usd_per_1m": "2.5",
            "output_usd_per_1m": "10"
        })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK, "upsert failed: {created}");
    assert_eq!(created["model_id"], json!("gpt-4o"));
    assert_eq!(created["source"], json!("manual"));
    assert_eq!(created["input_usd_per_1m"], json!("2.5"));
    let locked = created["locked_fields"].as_array().unwrap();
    assert!(locked.contains(&json!("input_usd_per_1m")));
    assert!(locked.contains(&json!("output_usd_per_1m")));

    // Merge semantics: omitted fields keep values, explicit null clears.
    let (status, updated) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/gpt-4o",
        Some(&admin),
        Some(json!({ "output_usd_per_1m": null })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    assert_eq!(updated["input_usd_per_1m"], json!("2.5"));
    assert_eq!(updated["output_usd_per_1m"], json!(null));

    // MP-Y18: replacing locked_fields explicitly removes locks.
    let (status, unlocked) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/gpt-4o",
        Some(&admin),
        Some(json!({ "locked_fields": [] })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    assert_eq!(unlocked["locked_fields"], json!([]));

    // MP-A2 validation: bad decimal rejects.
    let (status, err) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/gpt-4o",
        Some(&admin),
        Some(json!({ "input_usd_per_1m": "not-a-number" })),
    )
    .await;
    assert_eq!(status, MpStatusCode::BAD_REQUEST, "expected 400: {err}");

    // Wildcard route accepts model ids containing `/`.
    let (status, slashed) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/org/model-x",
        Some(&admin),
        Some(json!({ "billing_mode": "per_request", "per_request_usd": "0.04" })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    assert_eq!(slashed["model_id"], json!("org/model-x"));
    assert_eq!(slashed["billing_mode"], json!("per_request"));

    // MP-A1 ordering by model_id ASC.
    let (_, listed) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices",
        Some(&admin),
        None,
    )
    .await;
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["model_id"], json!("gpt-4o"));
    assert_eq!(rows[1]["model_id"], json!("org/model-x"));

    // MP-A3: delete, then 404 on repeat.
    let (status, _) = json_call(
        &ctx,
        MpMethod::DELETE,
        "/api/dashboard/model-prices/org/model-x",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    let (status, err) = json_call(
        &ctx,
        MpMethod::DELETE,
        "/api/dashboard/model-prices/org/model-x",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::NOT_FOUND, "expected 404: {err}");
}

#[tokio::test]
async fn model_price_endpoints_require_admin() {
    let ctx = setup().await;
    for (method, path) in [
        (MpMethod::GET, "/api/dashboard/model-prices"),
        (MpMethod::GET, "/api/dashboard/model-prices/unpriced"),
        (MpMethod::GET, "/api/dashboard/price-sync/runs"),
        (MpMethod::PUT, "/api/dashboard/model-prices/gpt-4o"),
        (MpMethod::POST, "/api/dashboard/price-sync/new_api/preview"),
        (MpMethod::POST, "/api/dashboard/price-sync/new_api/apply"),
    ] {
        let body = if method == MpMethod::PUT {
            Some(json!({}))
        } else {
            None
        };
        let (status, _) = json_call(&ctx, method.clone(), path, None, body).await;
        assert_eq!(
            status,
            MpStatusCode::UNAUTHORIZED,
            "expected 401 for {method} {path}"
        );
    }
}

#[tokio::test]
async fn tiered_expr_validation_rejects_bad_tiers() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_tiers").await;

    // MP-C10: structural violations reject at write time.
    let (status, err) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/tiered-model",
        Some(&admin),
        Some(json!({
            "billing_mode": "tiered_expr",
            "billing_expr": { "tiers": [] }
        })),
    )
    .await;
    assert_eq!(status, MpStatusCode::BAD_REQUEST, "expected 400: {err}");

    let (status, created) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/tiered-model",
        Some(&admin),
        Some(json!({
            "billing_mode": "tiered_expr",
            "billing_expr": { "tiers": [
                { "when_input_tokens_lte": 200000,
                  "input_usd_per_1m": "1.25", "output_usd_per_1m": "10" },
                { "input_usd_per_1m": "2.5", "output_usd_per_1m": "15" }
            ] }
        })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK, "tiered upsert failed: {created}");
    assert_eq!(created["billing_mode"], json!("tiered_expr"));
}

#[tokio::test]
async fn unpriced_models_reports_missing_pricing_keys() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_unpriced").await;
    create_test_provider(
        &ctx.state,
        "unpriced-alpha",
        monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        "alpha-model",
        "http://127.0.0.1:1/v1",
        "test-key",
    )
    .await;
    create_test_provider(
        &ctx.state,
        "unpriced-beta",
        monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        "beta-model",
        "http://127.0.0.1:1/v1",
        "test-key",
    )
    .await;

    let (status, body) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices/unpriced",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    let models: Vec<String> = body["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(models.contains(&"alpha-model".to_string()), "{models:?}");
    assert!(models.contains(&"beta-model".to_string()), "{models:?}");

    // Pricing one model removes it from the unpriced set (MP-A4).
    let (status, _) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/alpha-model",
        Some(&admin),
        Some(json!({ "input_usd_per_1m": "1" })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    let (_, body) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices/unpriced",
        Some(&admin),
        None,
    )
    .await;
    let models: Vec<String> = body["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!models.contains(&"alpha-model".to_string()), "{models:?}");
    assert!(models.contains(&"beta-model".to_string()), "{models:?}");
}

#[tokio::test]
async fn price_sync_runs_lists_recent_runs() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_runs").await;

    let (status, body) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/price-sync/runs",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn price_sync_rejects_unknown_source() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_badsource").await;

    let (status, err) = json_call(
        &ctx,
        MpMethod::POST,
        "/api/dashboard/price-sync/unknown/preview",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::BAD_REQUEST, "expected 400: {err}");
}

#[tokio::test]
async fn new_api_sync_requires_configured_base_url() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_newapi_unset").await;

    // MP-Y2: empty base URL means the source is disabled.
    let (status, err) = json_call(
        &ctx,
        MpMethod::POST,
        "/api/dashboard/price-sync/new_api/preview",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::BAD_REQUEST, "expected 400: {err}");
}

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

    // MP-A1: the fixture-seeded baseline holds no rows for this test's ids.
    let (status, body) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    let baseline = body.as_array().unwrap().len();
    assert!(
        !body
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["model_id"] == json!("gpt-4o")),
        "unexpected fixture row"
    );

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
    assert_eq!(rows.len(), baseline + 2);
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["model_id"].as_str().unwrap())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "rows must be ordered by model_id ASC");
    assert!(ids.contains(&"gpt-4o"));
    assert!(ids.contains(&"org/model-x"));

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

/// Starts a mock new-api pricing server and returns its base URL.
async fn start_mock_new_api(pricing: Value) -> String {
    let router = axum::Router::new().route(
        "/api/pricing",
        axum::routing::get(move || {
            let pricing = pricing.clone();
            async move { axum::Json(pricing) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn new_api_sync_preview_and_apply_end_to_end() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_newapi").await;

    // MP-Y12/MP-Y12a fixture: one ratio model, one fixed-price model, one
    // untrusted placeholder (75 USD/1M + completion ratio 1).
    let base_url = start_mock_new_api(json!({ "data": [
        { "model_name": "ratio-model", "quota_type": 0,
          "model_ratio": 1.25, "completion_ratio": 4 },
        { "model_name": "fixed-model", "quota_type": 1, "model_price": 0.05 },
        { "model_name": "placeholder-model", "quota_type": 0,
          "model_ratio": 37.5, "completion_ratio": 1 },
        { "model_name": "manual-owned-model", "quota_type": 0,
          "model_ratio": 5, "completion_ratio": 2 }
    ], "success": true }))
    .await;
    ctx.state
        .settings_store
        .set("price_sync_new_api_base_url", &base_url)
        .await
        .expect("set base url");

    // A pre-existing manual row is never modified by sync (MP-Y13).
    let (status, _) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/manual-owned-model",
        Some(&admin),
        Some(json!({ "input_usd_per_1m": "123", "output_usd_per_1m": "456" })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);

    // MP-A6 preview: no writes yet.
    let (status, preview) = json_call(
        &ctx,
        MpMethod::POST,
        "/api/dashboard/price-sync/new_api/preview",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK, "preview failed: {preview}");
    assert_eq!(preview["source"], json!("new_api"));
    assert_eq!(preview["insert"], json!(2));
    assert_eq!(preview["update"], json!(0));
    assert_eq!(preview["skip"], json!(1));
    assert_eq!(preview["delete"], json!(0));
    let (_, rows) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices",
        Some(&admin),
        None,
    )
    .await;
    assert!(
        !rows
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["source"] == json!("new_api")),
        "preview must not write"
    );

    // MP-A7 apply: returns the finalized run row.
    let (status, run) = json_call(
        &ctx,
        MpMethod::POST,
        "/api/dashboard/price-sync/new_api/apply",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK, "apply failed: {run}");
    assert_eq!(run["source"], json!("new_api"));
    assert_eq!(run["status"], json!("success"));
    assert_eq!(run["inserted"], json!(2));
    assert_eq!(run["skipped"], json!(1));

    let (_, rows) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices",
        Some(&admin),
        None,
    )
    .await;
    let rows = rows.as_array().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row["source"] == json!("new_api"))
            .count(),
        2
    );
    let ratio = rows
        .iter()
        .find(|row| row["model_id"] == json!("ratio-model"))
        .unwrap();
    // ratio 1 = USD 2 per 1M: 1.25 * 2 = 2.5; output 2.5 * 4 = 10 (MP-Y12).
    assert_eq!(ratio["billing_mode"], json!("per_token"));
    assert_eq!(ratio["input_usd_per_1m"], json!("2.5"));
    assert_eq!(ratio["output_usd_per_1m"], json!("10"));
    assert_eq!(ratio["source"], json!("new_api"));
    let fixed = rows
        .iter()
        .find(|row| row["model_id"] == json!("fixed-model"))
        .unwrap();
    assert_eq!(fixed["billing_mode"], json!("per_request"));
    assert_eq!(fixed["per_request_usd"], json!("0.05"));
    let manual = rows
        .iter()
        .find(|row| row["model_id"] == json!("manual-owned-model"))
        .unwrap();
    assert_eq!(manual["source"], json!("manual"));
    assert_eq!(manual["input_usd_per_1m"], json!("123"));

    // MP-A5: the run is auditable.
    let (_, runs) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/price-sync/runs",
        Some(&admin),
        None,
    )
    .await;
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"], json!("success"));

    // MP-Y14: a locked field survives a re-sync while unlocked fields and
    // raw_json refresh.
    let (status, locked) = json_call(
        &ctx,
        MpMethod::PUT,
        "/api/dashboard/model-prices/ratio-model",
        Some(&admin),
        Some(json!({ "input_usd_per_1m": "7" })),
    )
    .await;
    assert_eq!(status, MpStatusCode::OK);
    // MP-Y17: a dashboard edit of an existing synced row keeps its source,
    // gaining only the lock entry.
    assert_eq!(locked["source"], json!("new_api"));
    assert!(
        locked["locked_fields"]
            .as_array()
            .unwrap()
            .contains(&json!("input_usd_per_1m"))
    );
    let (status, run) = json_call(
        &ctx,
        MpMethod::POST,
        "/api/dashboard/price-sync/new_api/apply",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::OK, "re-apply failed: {run}");
    let (_, rows) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/model-prices",
        Some(&admin),
        None,
    )
    .await;
    let ratio = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["model_id"] == json!("ratio-model"))
        .cloned()
        .unwrap();
    assert_eq!(ratio["input_usd_per_1m"], json!("7"));
}

#[tokio::test]
async fn new_api_sync_fetch_failure_returns_502_and_audits_failed_run() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_mp_newapi_fail").await;

    // A closed port: connection refused maps to upstream_fetch_failed (MP-Y3).
    ctx.state
        .settings_store
        .set("price_sync_new_api_base_url", "http://127.0.0.1:9")
        .await
        .expect("set base url");

    let (status, err) = json_call(
        &ctx,
        MpMethod::POST,
        "/api/dashboard/price-sync/new_api/apply",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, MpStatusCode::BAD_GATEWAY, "expected 502: {err}");
    assert_eq!(err["error"]["code"], json!("upstream_fetch_failed"));

    let (_, runs) = json_call(
        &ctx,
        MpMethod::GET,
        "/api/dashboard/price-sync/runs",
        Some(&admin),
        None,
    )
    .await;
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"], json!("failed"));
    assert!(runs[0]["error"].as_str().unwrap().contains("fetch_failed"));
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

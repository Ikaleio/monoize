use super::*;
use axum::http::HeaderMap;

async fn compatibility_get(
    router: &axum::Router,
    path: &str,
    header_name: &'static str,
    header_value: &str,
) -> (StatusCode, HeaderMap, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header(header_name, header_value)
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).expect("response body is JSON");
    (status, headers, body)
}

async fn test_user(ctx: &TestContext) -> monoize::users::User {
    ctx.state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("test user exists")
}

async fn set_test_user_balance(ctx: &TestContext, nano_usd: &str, unlimited: bool) {
    let user = test_user(ctx).await;
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some(nano_usd),
            Some(unlimited),
            None,
            None,
        )
        .await
        .expect("update test user balance");
}

#[tokio::test]
async fn balance_compatibility_endpoints_require_api_key_authentication() {
    let ctx = setup().await;

    for path in ["/api/codex/usage", "/user/balance"] {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = ctx.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "unauthorized", "{path}");
    }
}

#[tokio::test]
async fn finite_user_balance_uses_exact_codex_and_deepseek_shapes() {
    let ctx = setup().await;
    set_test_user_balance(&ctx, "1234567890", false).await;

    let (status, headers, codex) = compatibility_get(
        &ctx.router,
        "/api/codex/usage",
        "authorization",
        &ctx.auth_header,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("cache-control").and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        codex,
        json!({
            "plan_type": "unknown",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": null,
                "secondary_window": null
            },
            "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": "1.23456789"
            },
            "spend_control": null,
            "additional_rate_limits": null,
            "rate_limit_reached_type": null,
            "rate_limit_reset_credits": { "available_count": 0 }
        })
    );

    let token = ctx.auth_header.strip_prefix("Bearer ").unwrap();
    let (status, headers, deepseek) =
        compatibility_get(&ctx.router, "/user/balance", "x-api-key", token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("cache-control").and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        deepseek,
        json!({
            "is_available": true,
            "balance_infos": [{
                "currency": "USD",
                "total_balance": "1.23456789",
                "granted_balance": "0",
                "topped_up_balance": "1.23456789"
            }]
        })
    );
}

#[tokio::test]
async fn sub_account_key_reports_its_balance_instead_of_unlimited_owner_balance() {
    let ctx = setup().await;
    let user = test_user(&ctx).await;
    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "balance-compat-sub-account".to_string(),
                expires_in_days: None,
                sub_account_enabled: true,
                sub_account_balance_nano_usd: Some("2500000000".to_string()),
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                use_user_group: true,
                group_ids: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
                reasoning_envelope_enabled: true,
                request_capture_mode: monoize::users::RequestCaptureMode::Off,
                request_capture_retention: monoize::users::RequestCaptureRetention::default(),
            },
            true,
        )
        .await
        .expect("create sub-account key");
    let authorization = format!("Bearer {token}");

    let (status, _, codex) = compatibility_get(
        &ctx.router,
        "/api/codex/usage",
        "authorization",
        &authorization,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(codex["credits"]["unlimited"], false);
    assert_eq!(codex["credits"]["balance"], "2.5");

    let (status, _, deepseek) = compatibility_get(
        &ctx.router,
        "/user/balance",
        "authorization",
        &authorization,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deepseek["balance_infos"][0]["total_balance"], "2.5");
}

#[tokio::test]
async fn zero_balance_is_reported_without_applying_the_forwarding_balance_gate() {
    let ctx = setup().await;
    set_test_user_balance(&ctx, "0", false).await;

    let (status, _, codex) = compatibility_get(
        &ctx.router,
        "/api/codex/usage",
        "authorization",
        &ctx.auth_header,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(codex["rate_limit"]["allowed"], false);
    assert_eq!(codex["rate_limit"]["limit_reached"], true);
    assert_eq!(codex["credits"]["has_credits"], false);
    assert_eq!(codex["credits"]["balance"], "0");
    assert_eq!(
        codex["rate_limit_reached_type"],
        json!({ "type": "rate_limit_reached" })
    );

    let (status, _, deepseek) = compatibility_get(
        &ctx.router,
        "/user/balance",
        "authorization",
        &ctx.auth_header,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deepseek["is_available"], false);
    assert_eq!(deepseek["balance_infos"][0]["total_balance"], "0");
}

#[tokio::test]
async fn unlimited_user_is_available_without_a_finite_codex_credit_balance() {
    let ctx = setup().await;
    set_test_user_balance(&ctx, "-500000000", true).await;

    let (status, _, codex) = compatibility_get(
        &ctx.router,
        "/api/codex/usage",
        "authorization",
        &ctx.auth_header,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(codex["rate_limit"]["allowed"], true);
    assert_eq!(codex["credits"]["has_credits"], false);
    assert_eq!(codex["credits"]["unlimited"], true);
    assert_eq!(codex["credits"]["balance"], Value::Null);

    let (status, _, deepseek) = compatibility_get(
        &ctx.router,
        "/user/balance",
        "authorization",
        &ctx.auth_header,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deepseek["is_available"], true);
    assert_eq!(deepseek["balance_infos"][0]["total_balance"], "-0.5");
}

#[tokio::test]
async fn replica_routes_subtract_pending_deductions_and_do_not_add_aliases() {
    let ctx = setup().await;
    set_test_user_balance(&ctx, "1000000000", false).await;
    let user = test_user(&ctx).await;

    let metering = monoize::replica::metering::ReplicaMetering::new(
        ctx._temp_dir.path().join("balance-compat-metering"),
        1024 * 1024,
        "http://127.0.0.1:9",
        "replica-token",
        100,
    )
    .expect("create replica metering");
    metering
        .enqueue_balance_delta(
            "request_charge",
            &user.id,
            None,
            400000000,
            &json!({ "request_id": "pending-balance-test" }),
        )
        .await
        .expect("enqueue pending deduction");

    let mut state = ctx.state.clone();
    let mut node = monoize::node_config::NodeSettings::primary_default();
    node.role = monoize::node_config::NodeRole::Replica;
    state.node = Arc::new(node);
    state.metering = Some(Arc::new(metering));
    let router = monoize::app::build_app(state);

    let (status, _, codex) = compatibility_get(
        &router,
        "/api/codex/usage",
        "authorization",
        &ctx.auth_header,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(codex["credits"]["balance"], "0.6");

    let (status, _, deepseek) =
        compatibility_get(&router, "/user/balance", "authorization", &ctx.auth_header).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deepseek["balance_infos"][0]["total_balance"], "0.6");

    for path in ["/codex/usage", "/api/user/balance"] {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .header(AUTHORIZATION, ctx.auth_header.clone())
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

use super::*;

// Tests for `GET /api/dashboard/me/live-usage` (user-live-usage.spec.md LU-1..LU-8).

fn live_usage_log(
    user_id: &str,
    request_id: &str,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    created_at: chrono::DateTime<chrono::Utc>,
) -> monoize::users::InsertRequestLog {
    monoize::users::InsertRequestLog {
        request_id: Some(request_id.to_string()),
        user_id: user_id.to_string(),
        api_key_id: None,
        model: "gpt-5-mini".to_string(),
        provider_id: None,
        upstream_model: None,
        channel_id: None,
        names: monoize::users::RequestLogNameSnapshots::default(),
        is_stream: false,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: None,
        charge_nano_usd: None,
        status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: Some(20),
        ttfb_ms: None,
        request_ip: Some("127.0.0.1".to_string()),
        reasoning_effort: None,
        tried_providers_json: None,
        effective_provider_type: None,
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        request_kind: None,
        created_at,
    }
}

async fn get_live_usage_with_cookie(ctx: &TestContext, cookie: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/api/dashboard/me/live-usage")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn live_usage_requires_authentication() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/dashboard/me/live-usage")
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn live_usage_empty_window_returns_zeros_and_null_rate() {
    let ctx = setup().await;
    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;
    let (status, body) = get_live_usage_with_cookie(&ctx, &cookie).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["window_seconds"], json!(60));
    assert_eq!(body["rpm"], json!(0));
    assert_eq!(body["tpm"], json!(0));
    assert_eq!(body["input_tokens"], json!(0));
    assert_eq!(body["output_tokens"], json!(0));
    assert_eq!(body["cache_read_tokens"], json!(0));
    assert_eq!(body["cache_hit_rate"], Value::Null);
}

#[tokio::test]
async fn live_usage_aggregates_window_rows_and_excludes_old_rows() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");
    let now = chrono::Utc::now();

    // Two in-window rows; null token fields count as 0 (LU-6).
    for log in [
        live_usage_log(&user.id, "lu-row-1", Some(100), Some(40), Some(25), now),
        live_usage_log(&user.id, "lu-row-2", None, Some(10), None, now),
        // Outside the 60-second window: must not count (LU-4).
        live_usage_log(
            &user.id,
            "lu-row-old",
            Some(9999),
            Some(9999),
            Some(9999),
            now - ChronoDuration::seconds(120),
        ),
    ] {
        ctx.state
            .user_store
            .finalize_request_log(log)
            .await
            .expect("enqueue request log");
    }
    ctx.state.user_store.flush_all_batchers().await;

    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;
    let (status, body) = get_live_usage_with_cookie(&ctx, &cookie).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["window_seconds"], json!(60));
    assert_eq!(body["rpm"], json!(2));
    assert_eq!(body["input_tokens"], json!(100));
    assert_eq!(body["output_tokens"], json!(50));
    assert_eq!(body["tpm"], json!(150));
    assert_eq!(body["cache_read_tokens"], json!(25));
    let rate = body["cache_hit_rate"].as_f64().expect("computed rate");
    assert!((rate - 0.25).abs() < 1e-12, "rate was {rate}");
}

#[tokio::test]
async fn live_usage_rate_is_null_when_window_has_no_input_tokens() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    ctx.state
        .user_store
        .finalize_request_log(live_usage_log(
            &user.id,
            "lu-output-only",
            None,
            Some(30),
            None,
            chrono::Utc::now(),
        ))
        .await
        .expect("enqueue request log");
    ctx.state.user_store.flush_all_batchers().await;

    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;
    let (status, body) = get_live_usage_with_cookie(&ctx, &cookie).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rpm"], json!(1));
    assert_eq!(body["tpm"], json!(30));
    assert_eq!(body["input_tokens"], json!(0));
    assert_eq!(
        body["cache_hit_rate"],
        Value::Null,
        "zero input tokens must yield null, not 0%"
    );
}

#[tokio::test]
async fn live_usage_is_scoped_to_the_session_user_even_for_admins() {
    let ctx = setup().await;
    let tenant = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");
    ctx.state
        .user_store
        .create_user(
            "admin_live",
            "test-password",
            monoize::users::UserRole::Admin,
            None,
        )
        .await
        .expect("create admin user");

    ctx.state
        .user_store
        .finalize_request_log(live_usage_log(
            &tenant.id,
            "lu-tenant-row",
            Some(50),
            Some(5),
            Some(10),
            chrono::Utc::now(),
        ))
        .await
        .expect("enqueue request log");
    ctx.state.user_store.flush_all_batchers().await;

    // The admin sees only their own (empty) window, never tenant-1's rows (LU-3).
    let admin_cookie = dashboard_session_cookie(&ctx, "admin_live", "test-password").await;
    let (status, body) = get_live_usage_with_cookie(&ctx, &admin_cookie).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rpm"], json!(0));
    assert_eq!(body["tpm"], json!(0));
    assert_eq!(body["cache_hit_rate"], Value::Null);

    // A user_id query parameter must not widen the scope (LU-3).
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/dashboard/me/live-usage?user_id={}",
            tenant.id
        ))
        .header("cookie", admin_cookie)
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let scoped: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(scoped["rpm"], json!(0));

    let tenant_cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;
    let (status, body) = get_live_usage_with_cookie(&ctx, &tenant_cookie).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rpm"], json!(1));
    assert_eq!(body["input_tokens"], json!(50));
    assert_eq!(body["tpm"], json!(55));
    let rate = body["cache_hit_rate"].as_f64().expect("computed rate");
    assert!((rate - 0.2).abs() < 1e-12, "rate was {rate}");
}

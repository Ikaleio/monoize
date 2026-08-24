use super::*;

// Mirrors the leak reported for Cloudflare AI gateway 502s: upstream URL,
// account id, internal hostname, and internal IP inside an unparseable body.
const LEAKY_RAW_BODY: &str = "upstream connect error for https://api.cloudflare.com/client/v4/accounts/ebb3b05a7371fbcbd62bde8264c86cfe/ai/v1/chat/completions: 502 Bad Gateway from cf-gateway-internal.example.net (10.32.4.17)";

fn assert_no_infra_leak(text: &str) {
    assert!(!text.contains("cloudflare"), "{text}");
    assert!(!text.contains("ebb3b05a7371fbcbd62bde8264c86cfe"), "{text}");
    assert!(!text.contains("cf-gateway-internal"), "{text}");
    assert!(!text.contains("10.32.4.17"), "{text}");
}

async fn find_error_log(ctx: &TestContext, model: &str) -> monoize::users::RequestLogRow {
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some(model),
                Some("error"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        if let Some(log) = logs.into_iter().find(|log| log.status == "error") {
            return log;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("error request log should be inserted for model {model}");
}

#[tokio::test]
async fn unparsed_upstream_error_body_is_hidden_from_client_and_kept_raw_in_stored_log() {
    let ctx = setup().await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "leak test" }],
            "force_upstream_error_status": 502,
            "force_upstream_error_raw_body": LEAKY_RAW_BODY
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_no_infra_leak(&body);
    let error: Value = serde_json::from_str(&body).expect("error response JSON");
    assert_eq!(
        error["error"]["message"],
        json!(
            "All upstream attempts failed for model: gpt-5-mini-chat. Last error: upstream status 502 Bad Gateway"
        )
    );
    assert_eq!(error["error"]["code"], json!("upstream_error"));
    assert_eq!(error["error"]["upstream_status"], json!(502));

    // SAN-9: the stored request-log detail keeps the counted wrapper plus the
    // full raw upstream body for admin-tier reads, while the client message
    // stays generic.
    let log = find_error_log(&ctx, "gpt-5-mini-chat").await;
    let log_message = log.error.message.as_deref().expect("log error message");
    assert!(
        log_message.starts_with("All 1 upstream attempt(s) failed for model: gpt-5-mini-chat."),
        "{log_message}"
    );
    assert!(log_message.contains("api.cloudflare.com"), "{log_message}");
    assert!(
        log_message.contains("ebb3b05a7371fbcbd62bde8264c86cfe"),
        "{log_message}"
    );
    assert!(log_message.contains("10.32.4.17"), "{log_message}");

    // SAN-10: the stored per-attempt error carries the same raw detail.
    let tried = log.tried_providers.as_ref().expect("tried providers");
    let first_error = tried[0]["error"].as_str().expect("first attempt error");
    assert!(first_error.contains("api.cloudflare.com"), "{first_error}");
    assert!(
        !tried.to_string().contains("client_error"),
        "client_error must never persist: {tried}"
    );
}

#[tokio::test]
async fn transport_error_is_hidden_from_client() {
    let ctx = setup().await;

    // Bind and drop a listener so the port refuses connections.
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        listener.local_addr().expect("local addr").port()
    };
    create_test_provider(
        &ctx.state,
        "up-dead",
        monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        "dead-model",
        &format!("http://127.0.0.1:{dead_port}"),
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["dead-model"]).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "dead-model",
            "messages": [{ "role": "user", "content": "transport failure" }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let error: Value = serde_json::from_str(&body).expect("error response JSON");
    assert_eq!(
        error["error"]["message"],
        json!(
            "All upstream attempts failed for model: dead-model. Last error: failed to request upstream"
        )
    );
    assert!(!body.contains("127.0.0.1"), "{body}");
    assert!(!body.contains(&dead_port.to_string()), "{body}");
    assert!(!body.contains("error sending request"), "{body}");

    // SAN-2/SAN-9: the stored detail keeps the raw transport text (with the
    // upstream address) for admin-tier reads.
    let log = find_error_log(&ctx, "dead-model").await;
    let log_message = log.error.message.as_deref().expect("log error message");
    assert!(log_message.contains("127.0.0.1"), "{log_message}");
}

#[tokio::test]
async fn structured_upstream_error_message_is_masked_but_passes_through() {
    let ctx = setup().await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "structured error" }],
            "force_upstream_error_status": 422,
            "force_upstream_error_code": "invalid_request_error",
            "force_upstream_error_message": "invalid request against https://api.cloudflare.com/client/v4/accounts/ebb3b05a7371fbcbd62bde8264c86cfe/ai"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_no_infra_leak(&body);
    let error: Value = serde_json::from_str(&body).expect("error response JSON");
    let message = error["error"]["message"].as_str().expect("error message");
    assert!(
        message.contains(
            "upstream status 422 Unprocessable Entity: invalid request against https://***.com/***"
        ),
        "{message}"
    );
    assert_eq!(error["error"]["code"], json!("invalid_request_error"));
}

async fn dashboard_get(ctx: &TestContext, path: &str, session_token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header(AUTHORIZATION, format!("Bearer {session_token}"))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("dashboard response JSON");
    (status, body)
}

async fn dashboard_put(
    ctx: &TestContext,
    path: &str,
    session_token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("PUT")
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {session_token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("dashboard response JSON");
    (status, body)
}

async fn create_admin_session(ctx: &TestContext, username: &str) -> String {
    let admin = ctx
        .state
        .user_store
        .create_user(
            username,
            "admin-password-12",
            monoize::users::UserRole::Admin,
            &[],
        )
        .await
        .expect("create admin user");
    ctx.state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("admin session")
        .token
}

/// SAN-CFG6: flip `monoize_mask_sensitive_info` through the real dashboard
/// settings endpoint so persistence and runtime publication are exercised.
async fn set_mask_sensitive_info(ctx: &TestContext, admin_token: &str, enabled: bool) {
    let (status, body) = dashboard_put(
        ctx,
        "/api/dashboard/settings",
        admin_token,
        json!({ "monoize_mask_sensitive_info": enabled }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["monoize_mask_sensitive_info"], json!(enabled));
    assert_eq!(
        ctx.state.monoize_runtime.read().await.mask_sensitive_info,
        enabled
    );
    // The PUT republishes the runtime from persisted settings, which drops
    // the whitelist that `setup()` injects directly into the runtime; restore
    // it so `force_upstream_error_*` test fields still reach the mock.
    configure_test_extra_fields_whitelist(&ctx.state).await;
}

async fn find_error_log_via_dashboard(
    ctx: &TestContext,
    session_token: &str,
    model: &str,
) -> Value {
    let path = format!("/api/dashboard/request-logs?status=error&model={model}");
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (status, body) = dashboard_get(ctx, &path, session_token).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if let Some(row) = body["data"].as_array().and_then(|rows| rows.first()) {
            return row.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("error request log should be listed for model {model}");
}

// RL-API14 / SAN-13 / SAN-14: the same stored row surfaces the full raw
// upstream detail to an admin session and the MASKed detail to the owning
// non-admin session.
#[tokio::test]
async fn request_log_error_detail_is_full_for_admin_and_masked_for_non_admin() {
    let ctx = setup().await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "role disclosure test" }],
            "force_upstream_error_status": 502,
            "force_upstream_error_raw_body": LEAKY_RAW_BODY
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_no_infra_leak(&body);

    let admin = ctx
        .state
        .user_store
        .create_user(
            "admin-full-detail",
            "admin-password-12",
            monoize::users::UserRole::Admin,
            &[],
        )
        .await
        .expect("create admin user");
    let admin_session = ctx
        .state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("admin session");

    let admin_row =
        find_error_log_via_dashboard(&ctx, &admin_session.token, "gpt-5-mini-chat").await;
    let admin_message = admin_row["error"]["message"]
        .as_str()
        .expect("admin error message");
    assert!(
        admin_message.contains("api.cloudflare.com"),
        "{admin_message}"
    );
    assert!(
        admin_message.contains("ebb3b05a7371fbcbd62bde8264c86cfe"),
        "{admin_message}"
    );
    assert!(admin_message.contains("10.32.4.17"), "{admin_message}");
    let admin_tried_error = admin_row["tried_providers"][0]["error"]
        .as_str()
        .expect("admin tried error");
    assert!(
        admin_tried_error.contains("api.cloudflare.com"),
        "{admin_tried_error}"
    );

    let tenant = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query tenant")
        .expect("tenant exists");
    let tenant_session = ctx
        .state
        .user_store
        .create_session(&tenant.id, 7)
        .await
        .expect("tenant session");

    let tenant_row =
        find_error_log_via_dashboard(&ctx, &tenant_session.token, "gpt-5-mini-chat").await;
    let tenant_message = tenant_row["error"]["message"]
        .as_str()
        .expect("tenant error message");
    assert_no_infra_leak(tenant_message);
    assert!(
        tenant_message.contains("https://***.com/***"),
        "{tenant_message}"
    );
    let tenant_tried_error = tenant_row["tried_providers"][0]["error"]
        .as_str()
        .expect("tenant tried error");
    assert_no_infra_leak(tenant_tried_error);
    assert!(
        tenant_tried_error.contains("https://***.com/***"),
        "{tenant_tried_error}"
    );
}

// SAN-CFG1/SAN-CFG2/SAN-CFG6: the boolean defaults to true, persists through
// PUT, and publishes to `monoize_runtime` inside the settings transaction.
#[tokio::test]
async fn mask_sensitive_info_setting_round_trips_and_publishes_runtime() {
    let ctx = setup().await;
    let admin_token = create_admin_session(&ctx, "admin-mask-roundtrip").await;

    let (status, body) = dashboard_get(&ctx, "/api/dashboard/settings", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["monoize_mask_sensitive_info"], json!(true));
    assert!(ctx.state.monoize_runtime.read().await.mask_sensitive_info);

    set_mask_sensitive_info(&ctx, &admin_token, false).await;
    let (status, body) = dashboard_get(&ctx, "/api/dashboard/settings", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["monoize_mask_sensitive_info"], json!(false));
    let stored = ctx
        .state
        .settings_store
        .get_all()
        .await
        .expect("settings load");
    assert!(!stored.monoize_mask_sensitive_info);

    set_mask_sensitive_info(&ctx, &admin_token, true).await;
    let stored = ctx
        .state
        .settings_store
        .get_all()
        .await
        .expect("settings load");
    assert!(stored.monoize_mask_sensitive_info);
}

// SAN-CFG5 item 3: with masking disabled, the unparsed upstream error body is
// forwarded to the client after the status prefix, TRUNC-bounded.
#[tokio::test]
async fn unparsed_error_body_reaches_client_when_masking_disabled() {
    let ctx = setup().await;
    let admin_token = create_admin_session(&ctx, "admin-mask-off-unparsed").await;
    set_mask_sensitive_info(&ctx, &admin_token, false).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "mask off leak test" }],
            "force_upstream_error_status": 502,
            "force_upstream_error_raw_body": LEAKY_RAW_BODY
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let error: Value = serde_json::from_str(&body).expect("error response JSON");
    assert_eq!(
        error["error"]["message"],
        json!(format!(
            "All upstream attempts failed for model: gpt-5-mini-chat. Last error: upstream status 502 Bad Gateway: {LEAKY_RAW_BODY}"
        ))
    );
}

// SAN-CFG5 item 1: with masking disabled, the structured upstream message is
// forwarded verbatim (no MASK).
#[tokio::test]
async fn structured_error_message_is_not_masked_when_masking_disabled() {
    let ctx = setup().await;
    let admin_token = create_admin_session(&ctx, "admin-mask-off-structured").await;
    set_mask_sensitive_info(&ctx, &admin_token, false).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "structured unmasked" }],
            "force_upstream_error_status": 422,
            "force_upstream_error_code": "invalid_request_error",
            "force_upstream_error_message": "invalid request against https://api.cloudflare.com/client/v4/accounts/ebb3b05a7371fbcbd62bde8264c86cfe/ai"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let error: Value = serde_json::from_str(&body).expect("error response JSON");
    let message = error["error"]["message"].as_str().expect("error message");
    assert!(
        message.contains(
            "upstream status 422 Unprocessable Entity: invalid request against https://api.cloudflare.com/client/v4/accounts/ebb3b05a7371fbcbd62bde8264c86cfe/ai"
        ),
        "{message}"
    );
}

// SAN-CFG5 item 2: with masking disabled, the transport error text (with the
// upstream address) is forwarded to the client after the status prefix.
#[tokio::test]
async fn transport_error_detail_reaches_client_when_masking_disabled() {
    let ctx = setup().await;
    let admin_token = create_admin_session(&ctx, "admin-mask-off-transport").await;
    set_mask_sensitive_info(&ctx, &admin_token, false).await;

    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        listener.local_addr().expect("local addr").port()
    };
    create_test_provider(
        &ctx.state,
        "up-dead-unmasked",
        monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
        "dead-model-unmasked",
        &format!("http://127.0.0.1:{dead_port}"),
        "upstream-key",
    )
    .await;
    seed_test_model_pricing(&ctx.state, &["dead-model-unmasked"]).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "dead-model-unmasked",
            "messages": [{ "role": "user", "content": "transport failure unmasked" }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let error: Value = serde_json::from_str(&body).expect("error response JSON");
    let message = error["error"]["message"].as_str().expect("error message");
    assert!(
        message.contains("upstream status 502 Bad Gateway: "),
        "{message}"
    );
    assert!(message.contains("127.0.0.1"), "{message}");
}

// SAN-CFG5 item 5: with masking disabled, the non-admin dashboard read
// returns the stored admin-tier text verbatim.
#[tokio::test]
async fn non_admin_request_log_read_skips_mask_when_masking_disabled() {
    let ctx = setup().await;
    let admin_token = create_admin_session(&ctx, "admin-mask-off-logs").await;
    set_mask_sensitive_info(&ctx, &admin_token, false).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "non-admin unmasked read" }],
            "force_upstream_error_status": 502,
            "force_upstream_error_raw_body": LEAKY_RAW_BODY
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");

    let tenant = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query tenant")
        .expect("tenant exists");
    let tenant_session = ctx
        .state
        .user_store
        .create_session(&tenant.id, 7)
        .await
        .expect("tenant session");

    let tenant_row =
        find_error_log_via_dashboard(&ctx, &tenant_session.token, "gpt-5-mini-chat").await;
    let tenant_message = tenant_row["error"]["message"]
        .as_str()
        .expect("tenant error message");
    assert!(
        tenant_message.contains("api.cloudflare.com"),
        "{tenant_message}"
    );
    assert!(
        tenant_message.contains("ebb3b05a7371fbcbd62bde8264c86cfe"),
        "{tenant_message}"
    );
    let tenant_tried_error = tenant_row["tried_providers"][0]["error"]
        .as_str()
        .expect("tenant tried error");
    assert!(
        tenant_tried_error.contains("api.cloudflare.com"),
        "{tenant_tried_error}"
    );
}

#[tokio::test]
async fn streaming_prestream_unparsed_error_body_is_hidden_from_client() {
    let ctx = setup().await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-chat",
                "messages": [{ "role": "user", "content": "stream leak test" }],
                "stream": true,
                "force_upstream_error_status": 502,
                "force_upstream_error_raw_body": LEAKY_RAW_BODY
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sse = String::from_utf8_lossy(&bytes).to_string();
    assert_no_infra_leak(&sse);
    assert!(
        sse.contains("upstream status 502 Bad Gateway"),
        "terminal stream error frame must carry the sanitized message: {sse}"
    );
}

// SAN-CFG5: with masking disabled, the streaming terminal error frame carries
// the raw upstream body.
#[tokio::test]
async fn streaming_prestream_unparsed_error_body_reaches_client_when_masking_disabled() {
    let ctx = setup().await;
    let admin_token = create_admin_session(&ctx, "admin-mask-off-stream").await;
    set_mask_sensitive_info(&ctx, &admin_token, false).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model": "gpt-5-mini-chat",
                "messages": [{ "role": "user", "content": "stream unmasked leak test" }],
                "stream": true,
                "force_upstream_error_status": 502,
                "force_upstream_error_raw_body": LEAKY_RAW_BODY
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sse = String::from_utf8_lossy(&bytes).to_string();
    assert!(sse.contains("api.cloudflare.com"), "{sse}");
    assert!(sse.contains("ebb3b05a7371fbcbd62bde8264c86cfe"), "{sse}");
}

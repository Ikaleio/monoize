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

async fn find_error_log(
    ctx: &TestContext,
    model: &str,
) -> monoize::users::RequestLogRow {
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
            .list_request_logs_by_user(&user.id, 100, 0, Some(model), Some("error"), None, None, None, None)
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
async fn unparsed_upstream_error_body_is_hidden_from_client_and_masked_in_logs() {
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

    // SAN-9: operators keep the masked, counted detail in the request log
    // while the client message stays generic. (SAN-10 tried_providers_json
    // content is asserted at unit level in
    // handlers::tests::exhausted_error_message_omits_attempt_count_and_infra_detail;
    // the list endpoints do not select that column.)
    let log = find_error_log(&ctx, "gpt-5-mini-chat").await;
    let log_message = log.error.message.as_deref().expect("log error message");
    assert!(
        log_message.starts_with("All 1 upstream attempt(s) failed for model: gpt-5-mini-chat."),
        "{log_message}"
    );
    assert!(log_message.contains("https://***.com/***"), "{log_message}");
    assert_no_infra_leak(log_message);
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

    let log = find_error_log(&ctx, "dead-model").await;
    let log_message = log.error.message.as_deref().expect("log error message");
    assert!(!log_message.contains("127.0.0.1"), "{log_message}");
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

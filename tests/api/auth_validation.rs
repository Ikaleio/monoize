use super::*;

#[tokio::test]
async fn auth_required_for_forwarding_endpoints() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_api_key_rejects_disallowed_transform() {
    let ctx = setup().await;
    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/dashboard/tokens")
        .header(CONTENT_TYPE, "application/json")
        .header("cookie", cookie)
        .body(Body::from(
            json!({
                "name": "unsafe-transform-key",
                "transforms": [
                    {
                        "transform": "set_field",
                        "enabled": true,
                        "models": ["gpt-5.4-fast"],
                        "phase": "request",
                        "config": {
                            "path": "service_tier",
                            "value": "priority"
                        }
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
}

#[tokio::test]
async fn create_api_key_allows_new_response_transforms() {
    let ctx = setup().await;
    let cookie = dashboard_session_cookie(&ctx, "tenant-1", "test-password").await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/dashboard/tokens")
        .header(CONTENT_TYPE, "application/json")
        .header("cookie", cookie)
        .body(Body::from(
            json!({
                "name": "safe-transform-key",
                "transforms": [
                    {
                        "transform": "plaintext_reasoning_to_summary",
                        "enabled": true,
                        "phase": "response",
                        "config": {}
                    },
                    {
                        "transform": "assistant_markdown_images_to_output",
                        "enabled": true,
                        "phase": "response",
                        "config": {}
                    },
                    {
                        "transform": "assistant_output_images_to_markdown",
                        "enabled": true,
                        "phase": "response",
                        "config": { "template": "![preview]({{src}})" }
                    }
                ]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let transforms = v["transforms"].as_array().expect("transforms array");
    assert_eq!(transforms.len(), 3);
}

#[tokio::test]
async fn auth_missing_authorization_header() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing auth"));
}

#[tokio::test]
async fn auth_accepts_x_api_key_header() {
    let ctx = setup().await;
    let token = ctx
        .auth_header
        .strip_prefix("Bearer ")
        .expect("bearer token");
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header("x-api-key", token)
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["output"][0]["content"][0]["text"].as_str(), Some("hi"));
}

#[tokio::test]
async fn auth_no_bearer_prefix() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Token sk-test123456")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid auth"));
}

#[tokio::test]
async fn auth_short_token() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer sk-short")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid token"));
}

#[tokio::test]
async fn auth_invalid_token_format() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer not-starting-with-sk-xxxx")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid token"));
}

#[tokio::test]
async fn auth_nonexistent_valid_format_token() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer sk-doesnotexistindb")
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"hi"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("unauthorized"));
    assert_eq!(v["error"]["message"].as_str(), Some("invalid token"));
}

#[tokio::test]
async fn body_not_json_returns_bad_request() {
    let ctx = setup().await;
    for path in ["/v1/responses", "/v1/chat/completions", "/v1/messages"] {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, ctx.auth_header.clone())
            .body(Body::from("this-is-not-json"))
            .unwrap();
        let resp = ctx.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn body_json_array_returns_bad_request() {
    let ctx = setup().await;
    for path in ["/v1/responses", "/v1/chat/completions", "/v1/messages"] {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, ctx.auth_header.clone())
            .body(Body::from("[1,2,3]"))
            .unwrap();
        let resp = ctx.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
        assert_eq!(v["error"]["message"].as_str(), Some("body must be object"));
    }
}

#[tokio::test]
async fn body_missing_model_returns_bad_request() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/responses", json!({"input":"hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn body_empty_model_returns_bad_request() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/embeddings", json!({"model":"","input":"hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn body_model_wrong_type_returns_bad_request() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/responses", json!({"model":123,"input":"hi"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn embeddings_missing_model() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/embeddings", json!({"input":"hello"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing model"));
}

#[tokio::test]
async fn embeddings_missing_input() {
    let ctx = setup().await;
    let (status, body) = json_post(&ctx, "/v1/embeddings", json!({"model":"gpt-5-mini"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(v["error"]["message"].as_str(), Some("missing input"));
}

#[tokio::test]
async fn embeddings_invalid_input_type() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/embeddings",
        json!({"model":"gpt-5-mini","input":123}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(
        v["error"]["message"].as_str(),
        Some("input must be string or array of strings")
    );
}

#[tokio::test]
async fn embeddings_invalid_encoding_format() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/embeddings",
        json!({"model":"gpt-5-mini","input":"hi","encoding_format":"xml"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(
        v["error"]["message"].as_str(),
        Some("encoding_format must be 'float' or 'base64'")
    );
}

#[tokio::test]
async fn embeddings_encoding_format_wrong_type() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/embeddings",
        json!({"model":"gpt-5-mini","input":"hi","encoding_format":42}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
    assert_eq!(
        v["error"]["message"].as_str(),
        Some("encoding_format must be 'float' or 'base64'")
    );
}

#[tokio::test]
async fn quota_exhausted_returns_429() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "quota-zero-key".to_string(),
                expires_in_days: None,
                quota: Some(0),
                quota_unlimited: false,
                model_limits_enabled: false,
                model_limits: vec![],
                ip_whitelist: Vec::new(),

                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
            },
            false,
        )
        .await
        .expect("create quota api key");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"quota check"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("quota_exceeded"));
}

#[tokio::test]
async fn ip_whitelist_blocks_non_whitelisted() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "ip-restricted-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: vec![],
                ip_whitelist: vec!["192.168.1.1".to_string()],

                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
            },
            false,
        )
        .await
        .expect("create ip restricted api key");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"ip-check"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("ip_not_allowed"));
}

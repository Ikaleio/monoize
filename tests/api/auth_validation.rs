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
async fn dashboard_auth_requires_a_valid_cap_token() {
    let ctx = setup().await;

    for path in ["/api/dashboard/auth/login", "/api/dashboard/auth/register"] {
        let missing = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"username": "tenant-1", "password": "test-password"}).to_string(),
            ))
            .unwrap();
        let response = ctx.router.clone().oneshot(missing).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["error"]["code"], json!("captcha_required"));

        let invalid = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "username": "tenant-1",
                    "password": "test-password",
                    "captcha_token": "invalid-token"
                })
                .to_string(),
            ))
            .unwrap();
        let response = ctx.router.clone().oneshot(invalid).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["error"]["code"], json!("captcha_invalid"));
    }
}

#[tokio::test]
async fn dashboard_auth_uses_builtin_cap_without_external_configuration() {
    let ctx = setup().await;
    let mut state = ctx.state.clone();
    state.cap_verifier = monoize::captcha::CapVerifier::builtin();
    let router = monoize::app::build_app(state);

    for path in ["/api/dashboard/auth/login", "/api/dashboard/auth/register"] {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "username": "tenant-1",
                    "password": "test-password",
                    "captcha_token": "present-token"
                })
                .to_string(),
            ))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["error"]["code"], json!("captcha_invalid"));
    }

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/dashboard/settings/public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csp = response
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("connect-src 'self';"));
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["captcha_enabled"], json!(true));
    assert_eq!(body["cap_api_endpoint"], json!("/api/dashboard/captcha/"));

    let challenge = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/dashboard/captcha/challenge")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(challenge.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&challenge.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["challenge"], json!({"c": 50, "s": 32, "d": 3}));
    assert!(
        body["token"]
            .as_str()
            .is_some_and(|token| token.len() >= 32)
    );
}

#[tokio::test]
async fn disabled_captcha_allows_login_without_a_token() {
    let ctx = setup().await;
    ctx.state
        .user_store
        .create_user(
            "captcha-admin",
            "admin-password-12",
            monoize::users::UserRole::Admin,
            None,
        )
        .await
        .unwrap();
    let cookie = dashboard_session_cookie(&ctx, "captcha-admin", "admin-password-12").await;

    let update = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/dashboard/settings")
                .header(CONTENT_TYPE, "application/json")
                .header("cookie", &cookie)
                .body(Body::from(json!({"captcha_enabled": false}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);

    let me = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/dashboard/auth/me")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);

    let response = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/dashboard/auth/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": "tenant-1",
                        "password": "test-password"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let public = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/dashboard/settings/public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&public.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["captcha_enabled"], json!(false));
    assert!(body["cap_api_endpoint"].is_null());
}

#[tokio::test]
async fn dashboard_auth_missing_session_uses_session_error() {
    let ctx = setup().await;
    let response = ctx
        .router
        .oneshot(
            Request::builder()
                .uri("/api/dashboard/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"]["code"], json!("unauthorized"));
    assert_eq!(body["error"]["message"], json!("missing dashboard session"));
}

#[tokio::test]
async fn public_settings_and_csp_publish_only_cap_public_configuration() {
    let ctx = setup().await;
    let request = || {
        Request::builder()
            .method("GET")
            .uri("/api/dashboard/settings/public")
            .body(Body::empty())
            .unwrap()
    };

    let first = ctx.router.clone().oneshot(request()).await.unwrap();
    let first_csp = first
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(first_csp.contains("connect-src 'self' http://127.0.0.1:"));
    assert!(first_csp.contains("worker-src 'self' blob:"));
    assert!(first_csp.contains("script-src 'self' 'wasm-unsafe-eval' 'nonce-"));
    assert!(!first_csp.contains("script-src 'self' 'unsafe-inline'"));
    assert!(!first_csp.contains("'unsafe-eval'"));
    let body: Value =
        serde_json::from_slice(&first.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["captcha_enabled"], json!(true));
    assert!(
        body["cap_api_endpoint"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    assert!(!body.to_string().contains("test-cap-secret"));

    let second = ctx.router.clone().oneshot(request()).await.unwrap();
    let second_csp = second
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert_ne!(first_csp, second_csp);
}

#[tokio::test]
async fn password_change_rotates_current_session_and_revokes_other_sessions() {
    let ctx = setup().await;
    let login_request = || {
        Request::builder()
            .method("POST")
            .uri("/api/dashboard/auth/login")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "username": "tenant-1",
                    "password": "test-password",
                    "captcha_token": "test-captcha-token"
                })
                .to_string(),
            ))
            .unwrap()
    };

    let first_login = ctx.router.clone().oneshot(login_request()).await.unwrap();
    assert_eq!(first_login.status(), StatusCode::OK);
    let first_body: Value =
        serde_json::from_slice(&first_login.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let first_token = first_body["token"].as_str().unwrap().to_string();

    let second_login = ctx.router.clone().oneshot(login_request()).await.unwrap();
    assert_eq!(second_login.status(), StatusCode::OK);
    let second_body: Value =
        serde_json::from_slice(&second_login.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let second_token = second_body["token"].as_str().unwrap().to_string();

    let change = Request::builder()
        .method("PUT")
        .uri("/api/dashboard/auth/password")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {first_token}"))
        .body(Body::from(
            json!({
                "current_password": "test-password",
                "new_password": "changed-password"
            })
            .to_string(),
        ))
        .unwrap();
    let changed = ctx.router.clone().oneshot(change).await.unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert!(
        changed
            .headers()
            .get("set-cookie")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("monoize_session=urp_session_"))
    );
    let changed_body: Value =
        serde_json::from_slice(&changed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let replacement_token = changed_body["token"].as_str().unwrap().to_string();

    assert!(
        ctx.state
            .user_store
            .get_session_by_token(&first_token)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        ctx.state
            .user_store
            .get_session_by_token(&second_token)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        ctx.state
            .user_store
            .get_session_by_token(&replacement_token)
            .await
            .unwrap()
            .is_some()
    );

    let old_session_request = Request::builder()
        .uri("/api/dashboard/auth/me")
        .header(AUTHORIZATION, format!("Bearer {second_token}"))
        .body(Body::empty())
        .unwrap();
    let old_session_response = ctx
        .router
        .clone()
        .oneshot(old_session_request)
        .await
        .unwrap();
    assert_eq!(old_session_response.status(), StatusCode::UNAUTHORIZED);

    let replacement_request = Request::builder()
        .uri("/api/dashboard/auth/me")
        .header(AUTHORIZATION, format!("Bearer {replacement_token}"))
        .body(Body::empty())
        .unwrap();
    let replacement_response = ctx
        .router
        .clone()
        .oneshot(replacement_request)
        .await
        .unwrap();
    assert_eq!(replacement_response.status(), StatusCode::OK);

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .unwrap()
        .unwrap();
    assert!(
        monoize::users::UserStore::verify_password_async("changed-password", &user.password_hash,)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn password_change_rejects_an_incorrect_current_password_without_revocation() {
    let ctx = setup().await;
    let login = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/dashboard/auth/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "username": "tenant-1",
                        "password": "test-password",
                        "captcha_token": "test-captcha-token"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let login_body: Value =
        serde_json::from_slice(&login.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let token = login_body["token"].as_str().unwrap().to_string();

    let response = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/dashboard/auth/password")
                .header(CONTENT_TYPE, "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({
                        "current_password": "wrong-password",
                        "new_password": "changed-password"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"]["code"], json!("invalid_current_password"));
    assert!(
        ctx.state
            .user_store
            .get_session_by_token(&token)
            .await
            .unwrap()
            .is_some()
    );
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
                        "transform": "field_set",
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
                        "transform": "reasoning_content_to_summary",
                        "enabled": true,
                        "phase": "response",
                        "config": {}
                    },
                    {
                        "transform": "image_markdown_to_output",
                        "enabled": true,
                        "phase": "response",
                        "config": {}
                    },
                    {
                        "transform": "image_output_to_markdown",
                        "enabled": true,
                        "phase": "response",
                        "config": { "template": "![preview]({{src}})" }
                    },
                    {
                        "transform": "image_compress_output",
                        "enabled": true,
                        "phase": "response",
                        "config": { "max_edge_px": 1024, "jpeg_quality": 80 }
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
    assert_eq!(transforms.len(), 4);
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
        if path == "/v1/messages" {
            assert_eq!(v["type"].as_str(), Some("error"));
            assert_eq!(v["error"]["type"].as_str(), Some("invalid_request_error"));
            assert!(v["error"].get("code").is_none());
        } else {
            assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"));
        }
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
async fn sub_account_zero_balance_returns_402() {
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
                name: "sub-account-zero-key".to_string(),
                expires_in_days: None,
                sub_account_enabled: true,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: false,
                model_limits: vec![],
                ip_whitelist: Vec::new(),

                use_user_group: true,
                group_ids: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
                reasoning_envelope_enabled: true,
                request_capture_mode: monoize::users::RequestCaptureMode::Off,
            },
            false,
        )
        .await
        .expect("create sub-account api key");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"model":"gpt-5-mini","input":"sub-account check"}).to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("insufficient_balance"));
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
                sub_account_enabled: false,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: false,
                model_limits: vec![],
                ip_whitelist: vec!["192.168.1.1".to_string()],

                use_user_group: true,
                group_ids: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
                reasoning_envelope_enabled: true,
                request_capture_mode: monoize::users::RequestCaptureMode::Off,
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

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Method, Request, StatusCode};
use monoize::app::{AppState, RuntimeConfig, build_app, load_state_with_runtime};
use monoize::users::{UpdateApiKeyInput, UserRole, UserStore};
use serde_json::json;
use tower::ServiceExt;

async fn test_state() -> AppState {
    load_state_with_runtime(RuntimeConfig::with_defaults(
        "127.0.0.1:0",
        "/metrics",
        "sqlite::memory:".to_string(),
    ))
    .await
    .expect("test state loads")
}

fn empty_api_key_update(expires_at: Option<&str>) -> UpdateApiKeyInput {
    UpdateApiKeyInput {
        name: None,
        enabled: None,
        sub_account_enabled: None,
        sub_account_balance_nano_usd: None,
        model_limits_enabled: None,
        model_limits: None,
        ip_whitelist: None,
        use_user_group: None,
        group_ids: None,
        max_multiplier: None,
        transforms: None,
        model_redirects: None,
        reasoning_envelope_enabled: None,
        request_capture_mode: None,
        expires_at: expires_at.map(str::to_string),
    }
}

async fn request(
    router: &axum::Router,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    router
        .clone()
        .oneshot(builder.body(body).expect("request builds"))
        .await
        .expect("request completes")
}

#[tokio::test]
async fn topology_surfaces_require_an_admin_session() {
    let state = test_state().await;
    let admin = state
        .user_store
        .create_user("topology_admin", "password123", UserRole::Admin, None)
        .await
        .expect("admin creates");
    let user = state
        .user_store
        .create_user("topology_user", "password123", UserRole::User, None)
        .await
        .expect("user creates");
    let admin_session = state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("admin session creates");
    let user_session = state
        .user_store
        .create_session(&user.id, 7)
        .await
        .expect("user session creates");
    let router = build_app(state);

    for path in [
        "/metrics",
        "/presets/providers",
        "/presets/apikeys",
        "/api/dashboard/transforms/registry",
    ] {
        let anonymous = request(&router, Method::GET, path, None, None).await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{path}");

        let non_admin = request(&router, Method::GET, path, Some(&user_session.token), None).await;
        assert_eq!(non_admin.status(), StatusCode::FORBIDDEN, "{path}");

        let admin = request(&router, Method::GET, path, Some(&admin_session.token), None).await;
        assert_eq!(admin.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn api_key_expiry_update_rejects_invalid_rfc3339_before_writing() {
    let state = test_state().await;
    let user = state
        .user_store
        .create_user("expiry_user", "password123", UserRole::User, None)
        .await
        .expect("user creates");
    let (key, _) = state
        .user_store
        .create_api_key(&user.id, "original name", None)
        .await
        .expect("API key creates");
    let mut invalid_update = empty_api_key_update(Some("not-a-timestamp"));
    invalid_update.name = Some("must not be stored".to_string());

    let error = state
        .user_store
        .update_api_key(&key.id, invalid_update, false)
        .await
        .expect_err("invalid expiry is rejected");
    assert_eq!(error, "expires_at must be a valid RFC3339 timestamp");

    let unchanged = state
        .user_store
        .get_api_key_by_id(&key.id)
        .await
        .expect("API key lookup succeeds")
        .expect("API key remains present");
    assert_eq!(unchanged.name, "original name");
    assert_eq!(unchanged.expires_at, None);

    let supplied = "2030-01-02T03:04:05+08:00";
    let updated = state
        .user_store
        .update_api_key(&key.id, empty_api_key_update(Some(supplied)), false)
        .await
        .expect("valid expiry updates");
    let expected = chrono::DateTime::parse_from_rfc3339(supplied)
        .expect("test timestamp parses")
        .with_timezone(&chrono::Utc);
    assert_eq!(updated.expires_at, Some(expected));
}

#[tokio::test]
async fn admin_cannot_update_a_peer_admin_but_super_admin_can() {
    let state = test_state().await;
    let admin_a = state
        .user_store
        .create_user("admin_a", "password-a", UserRole::Admin, None)
        .await
        .expect("first admin creates");
    let admin_b = state
        .user_store
        .create_user("admin_b", "password-b", UserRole::Admin, None)
        .await
        .expect("second admin creates");
    let super_admin = state
        .user_store
        .create_user("super_admin", "password-s", UserRole::SuperAdmin, None)
        .await
        .expect("super admin creates");
    let admin_session = state
        .user_store
        .create_session(&admin_a.id, 7)
        .await
        .expect("admin session creates");
    let super_session = state
        .user_store
        .create_session(&super_admin.id, 7)
        .await
        .expect("super admin session creates");
    let router = build_app(state.clone());

    let peer_update = request(
        &router,
        Method::PUT,
        &format!("/api/dashboard/users/{}", admin_b.id),
        Some(&admin_session.token),
        Some(json!({ "password": "taken-over-password" })),
    )
    .await;
    assert_eq!(peer_update.status(), StatusCode::FORBIDDEN);
    let unchanged = state
        .user_store
        .get_user_by_id(&admin_b.id)
        .await
        .expect("peer lookup succeeds")
        .expect("peer remains present");
    assert!(
        UserStore::verify_password_async("password-b", &unchanged.password_hash)
            .await
            .expect("old password verifies")
    );
    assert!(
        !UserStore::verify_password_async("taken-over-password", &unchanged.password_hash)
            .await
            .expect("new password does not verify")
    );

    let self_update = request(
        &router,
        Method::PUT,
        &format!("/api/dashboard/users/{}", admin_a.id),
        Some(&admin_session.token),
        Some(json!({ "password": "self-password" })),
    )
    .await;
    assert_eq!(self_update.status(), StatusCode::OK);

    let super_update = request(
        &router,
        Method::PUT,
        &format!("/api/dashboard/users/{}", admin_b.id),
        Some(&super_session.token),
        Some(json!({ "password": "super-set-password" })),
    )
    .await;
    assert_eq!(super_update.status(), StatusCode::OK);
}

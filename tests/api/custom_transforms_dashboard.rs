//! HTTP integration tests for custom JS transforms
//! (`custom-js-transforms.spec.md` §5, §9, §10 and CJS-RT end-to-end).

use super::*;
use axum::body::Body as CtBody;
use axum::http::Method as CtMethod;
use axum::http::Request as CtRequest;
use axum::http::StatusCode as CtStatusCode;
use axum::http::header::{AUTHORIZATION as CT_AUTHORIZATION, CONTENT_TYPE as CT_CONTENT_TYPE};

const USER_VISIBLE_SOURCE: &str = r#"/**
 * @monoize-transform
 * id: js:echo-marker
 * name: Echo Marker
 * description: Marks requests with an echo field.
 * author: integration-test
 * phase: request
 * scopes: provider, global, api_key
 * visibility: user
 */
const configSchema = { type: "object", properties: { marker: { type: "string" } } };
function transform(ctx) {
  ctx.data.extra_echo = ctx.config.marker || "custom-js-applied";
}
"#;

const ADMIN_ONLY_SOURCE: &str = r#"/**
 * @monoize-transform
 * id: js:admin-secret
 * name: Admin Secret
 * description: Admin-only transform.
 * author: integration-test
 */
function transform(ctx) {}
"#;

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

async fn user_session_header(ctx: &TestContext, username: &str) -> String {
    let user = ctx
        .state
        .user_store
        .create_user(username, "password", monoize::users::UserRole::User, None)
        .await
        .expect("user created");
    let session = ctx
        .state
        .user_store
        .create_session(&user.id, 7)
        .await
        .expect("session created");
    format!("Bearer {}", session.token)
}

async fn json_call(
    ctx: &TestContext,
    method: CtMethod,
    path: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (CtStatusCode, Value) {
    let mut builder = CtRequest::builder().method(method).uri(path);
    if let Some(auth) = auth {
        builder = builder.header(CT_AUTHORIZATION, auth);
    }
    let body = if let Some(body) = body {
        builder = builder.header(CT_CONTENT_TYPE, "application/json");
        CtBody::from(body.to_string())
    } else {
        CtBody::empty()
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
async fn custom_transform_crud_lifecycle_and_error_codes() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_ct_crud").await;

    // Create.
    let (status, created) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/custom-transforms",
        Some(&admin),
        Some(json!({ "source": USER_VISIBLE_SOURCE })),
    )
    .await;
    assert_eq!(status, CtStatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["id"], json!("js:echo-marker"));
    assert_eq!(created["name"], json!("Echo Marker"));
    assert_eq!(created["enabled"], json!(true));
    assert_eq!(created["visibility"], json!("user"));
    assert_eq!(created["phases"], json!(["request"]));
    assert_eq!(
        created["config_schema"]["properties"]["marker"]["type"],
        json!("string")
    );

    // Duplicate id conflicts (CJS-VAL-6).
    let (status, body) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/custom-transforms",
        Some(&admin),
        Some(json!({ "source": USER_VISIBLE_SOURCE })),
    )
    .await;
    assert_eq!(status, CtStatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], json!("custom_transform_exists"));

    // Invalid source rejects with the spec error code (CJS-VAL-2).
    let (status, body) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/custom-transforms",
        Some(&admin),
        Some(json!({ "source": "function transform(ctx) {}" })),
    )
    .await;
    assert_eq!(status, CtStatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_custom_transform"));

    // List includes the row.
    let (status, listed) = json_call(
        &ctx,
        CtMethod::GET,
        "/api/dashboard/custom-transforms",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, CtStatusCode::OK);
    assert_eq!(listed["transforms"].as_array().unwrap().len(), 1);

    // Toggle enabled.
    let (status, updated) = json_call(
        &ctx,
        CtMethod::PUT,
        "/api/dashboard/custom-transforms/js:echo-marker",
        Some(&admin),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, CtStatusCode::OK);
    assert_eq!(updated["enabled"], json!(false));

    // Empty update body rejects (CJS-API-3).
    let (status, _) = json_call(
        &ctx,
        CtMethod::PUT,
        "/api/dashboard/custom-transforms/js:echo-marker",
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, CtStatusCode::BAD_REQUEST);

    // Frontmatter id must match the path id (CJS-VAL-5).
    let renamed = USER_VISIBLE_SOURCE.replace("js:echo-marker", "js:renamed");
    let (status, body) = json_call(
        &ctx,
        CtMethod::PUT,
        "/api/dashboard/custom-transforms/js:echo-marker",
        Some(&admin),
        Some(json!({ "source": renamed })),
    )
    .await;
    assert_eq!(status, CtStatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_custom_transform"));

    // Delete, then 404 on repeat.
    let (status, body) = json_call(
        &ctx,
        CtMethod::DELETE,
        "/api/dashboard/custom-transforms/js:echo-marker",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, CtStatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let (status, _) = json_call(
        &ctx,
        CtMethod::DELETE,
        "/api/dashboard/custom-transforms/js:echo-marker",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, CtStatusCode::NOT_FOUND);
}

#[tokio::test]
async fn custom_transform_endpoints_require_admin() {
    let ctx = setup().await;
    let user = user_session_header(&ctx, "plain_ct_user").await;

    for (method, path, body) in [
        (
            CtMethod::GET,
            "/api/dashboard/custom-transforms",
            None::<Value>,
        ),
        (
            CtMethod::POST,
            "/api/dashboard/custom-transforms",
            Some(json!({ "source": USER_VISIBLE_SOURCE })),
        ),
        (
            CtMethod::PUT,
            "/api/dashboard/custom-transforms/js:echo-marker",
            Some(json!({ "enabled": false })),
        ),
        (
            CtMethod::DELETE,
            "/api/dashboard/custom-transforms/js:echo-marker",
            None,
        ),
    ] {
        let (status, _) = json_call(&ctx, method.clone(), path, Some(&user), body.clone()).await;
        assert_eq!(status, CtStatusCode::FORBIDDEN, "{method} {path}");
        let (status, _) = json_call(&ctx, method.clone(), path, None, body).await;
        assert_eq!(status, CtStatusCode::UNAUTHORIZED, "{method} {path} anon");
    }
}

#[tokio::test]
async fn registry_visibility_filters_custom_transforms_by_caller() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_ct_registry").await;
    let user = user_session_header(&ctx, "user_ct_registry").await;

    for source in [USER_VISIBLE_SOURCE, ADMIN_ONLY_SOURCE] {
        let (status, _) = json_call(
            &ctx,
            CtMethod::POST,
            "/api/dashboard/custom-transforms",
            Some(&admin),
            Some(json!({ "source": source })),
        )
        .await;
        assert_eq!(status, CtStatusCode::CREATED);
    }

    let custom_ids = |items: &Value| -> Vec<String> {
        items
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["custom"] == json!(true))
            .map(|item| item["type_id"].as_str().unwrap().to_string())
            .collect()
    };

    // CJS-REG-1 + SAC-1: admin session sees every enabled custom transform.
    let (status, items) = json_call(
        &ctx,
        CtMethod::GET,
        "/api/dashboard/transforms/registry",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, CtStatusCode::OK);
    assert_eq!(custom_ids(&items), vec!["js:admin-secret", "js:echo-marker"]);
    let echo = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type_id"] == json!("js:echo-marker"))
        .expect("echo item");
    // CJS-REG-2: plain strings mirrored into both locale keys, marker fields set.
    assert_eq!(echo["name"]["en"], json!("Echo Marker"));
    assert_eq!(echo["name"]["zh"], json!("Echo Marker"));
    assert_eq!(echo["visibility"], json!("user"));
    assert_eq!(
        echo["supported_scopes"],
        json!(["provider", "global", "api_key"])
    );
    // CJS-REG-3: built-in items carry no custom/visibility markers.
    let builtin = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type_id"] == json!("field_set"))
        .expect("builtin item");
    assert!(builtin.get("custom").is_none());
    assert!(builtin.get("visibility").is_none());

    // SAC-3 / SAC-2: non-admin session is 403; missing session is 401.
    let (status, _) = json_call(
        &ctx,
        CtMethod::GET,
        "/api/dashboard/transforms/registry",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, CtStatusCode::FORBIDDEN);
    let (status, _) = json_call(
        &ctx,
        CtMethod::GET,
        "/api/dashboard/transforms/registry",
        None,
        None,
    )
    .await;
    assert_eq!(status, CtStatusCode::UNAUTHORIZED);

    // CJS-REG-4: disabled custom transforms disappear for every caller.
    let (status, _) = json_call(
        &ctx,
        CtMethod::PUT,
        "/api/dashboard/custom-transforms/js:echo-marker",
        Some(&admin),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, CtStatusCode::OK);
    let (_, items) = json_call(
        &ctx,
        CtMethod::GET,
        "/api/dashboard/transforms/registry",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(custom_ids(&items), vec!["js:admin-secret"]);
}

#[tokio::test]
async fn api_key_transforms_accept_user_visible_custom_rules_only() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_ct_apikey").await;
    let user = user_session_header(&ctx, "user_ct_apikey").await;

    for source in [USER_VISIBLE_SOURCE, ADMIN_ONLY_SOURCE] {
        let (status, _) = json_call(
            &ctx,
            CtMethod::POST,
            "/api/dashboard/custom-transforms",
            Some(&admin),
            Some(json!({ "source": source })),
        )
        .await;
        assert_eq!(status, CtStatusCode::CREATED);
    }

    let key_body = |name: &str, transform: &str| {
        json!({
            "name": name,
            "transforms": [{
                "transform": transform,
                "enabled": true,
                "phase": "request",
                "config": {}
            }]
        })
    };

    // CJS-AKV-2: user-visible + api_key scope + request phase passes.
    let (status, body) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/tokens",
        Some(&user),
        Some(key_body("with-custom", "js:echo-marker")),
    )
    .await;
    assert_eq!(status, CtStatusCode::CREATED, "allowed rule failed: {body}");

    // CJS-AKV-3: admin-only custom transforms reject for non-admin callers.
    let (status, body) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/tokens",
        Some(&user),
        Some(key_body("with-admin-only", "js:admin-secret")),
    )
    .await;
    assert_ne!(status, CtStatusCode::CREATED, "admin-only rule must fail");
    assert!(
        body.to_string().contains("not allowed"),
        "unexpected body: {body}"
    );

    // Admin callers bypass the gate (CJS-AKV-1).
    let (status, body) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/tokens",
        Some(&admin),
        Some(key_body("admin-key", "js:admin-secret")),
    )
    .await;
    assert_eq!(status, CtStatusCode::CREATED, "admin bypass failed: {body}");
}

#[tokio::test]
async fn custom_transform_rewrites_proxied_request_end_to_end() {
    let ctx = setup().await;
    let admin = admin_header(&ctx, "admin_ct_e2e").await;

    let (status, created) = json_call(
        &ctx,
        CtMethod::POST,
        "/api/dashboard/custom-transforms",
        Some(&admin),
        Some(json!({ "source": USER_VISIBLE_SOURCE })),
    )
    .await;
    assert_eq!(status, CtStatusCode::CREATED, "create failed: {created}");

    // Attach the custom transform to the chat provider chain (provider scope).
    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("providers list");
    let chat_provider = providers
        .iter()
        .find(|provider| provider.name == "up-chat")
        .expect("chat provider");
    let update: monoize::monoize_routing::UpdateMonoizeProviderInput = serde_json::from_value(
        json!({
            "transforms": [{
                "transform": "js:echo-marker",
                "enabled": true,
                "phase": "request",
                "config": { "marker": "custom-js-applied" }
            }]
        }),
    )
    .expect("update input");
    ctx.state
        .monoize_store
        .update_provider(&chat_provider.id, update)
        .await
        .expect("provider update");

    let (status, body) = json_call(
        &ctx,
        CtMethod::POST,
        "/v1/chat/completions",
        Some(&ctx.auth_header),
        Some(json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "hello" }]
        })),
    )
    .await;
    assert_eq!(status, CtStatusCode::OK, "proxy call failed: {body}");

    // CJS-JS-9 not exercised here; this asserts the CJS-JS-4 request-mutation
    // path: the sandbox set extra_echo and the upstream body carries it.
    let bodies = ctx.captured_bodies.lock().unwrap();
    let upstream_body = bodies
        .iter()
        .rev()
        .find(|(name, _)| name == "chat")
        .map(|(_, body)| body.clone())
        .expect("captured upstream chat body");
    assert_eq!(upstream_body["extra_echo"], json!("custom-js-applied"));
}

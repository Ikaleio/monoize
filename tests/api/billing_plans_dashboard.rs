use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use monoize::app::{RuntimeConfig, build_app, load_state_with_runtime};
use monoize::users::UserRole;
use serde_json::{Value, json};
use tower::ServiceExt;

struct TestContext {
    router: axum::Router,
    auth_header: String,
}

async fn setup() -> TestContext {
    let state = load_state_with_runtime(RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
    })
    .await
    .expect("state loads");
    let admin = state
        .user_store
        .create_user("admin_billing_plans", "password", UserRole::Admin, &[])
        .await
        .expect("admin created");
    let session = state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("session created");

    TestContext {
        router: build_app(state),
        auth_header: format!("Bearer {}", session.token),
    }
}

async fn json_request(
    ctx: &TestContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(AUTHORIZATION, ctx.auth_header.clone());
    let body = if let Some(body) = body {
        builder = builder.header(CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
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
async fn billing_plan_validation_and_assignment_error_codes() {
    let ctx = setup().await;

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "bad-period",
            "grant_amount_usd": "1",
            "period_seconds": 0
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_period"));

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "bad-amount",
            "grant_amount_nano_usd": "abc",
            "period_seconds": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_grant_amount"));

    let (status, created) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "zero",
            "grant_amount_usd": "0",
            "period_seconds": 60,
            "allowed_groups": ["team-a"],
            "enabled": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["grant_amount_nano_usd"], json!("0"));
    assert_eq!(created["enabled"], json!(false));
    assert_eq!(created["allowed_groups"], json!(["team-a"]));
    let plan_id = created["id"].as_str().expect("plan id").to_string();

    let (status, _) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/billing-plans/{plan_id}"),
        Some(json!({
            "name": "zero",
            "grant_amount_nano_usd": "0",
            "period_seconds": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, plans) =
        json_request(&ctx, Method::GET, "/api/dashboard/billing-plans", None).await;
    assert_eq!(status, StatusCode::OK);
    let kept = plans
        .as_array()
        .expect("plans array")
        .iter()
        .find(|plan| plan["id"] == json!(plan_id))
        .expect("created plan listed");
    assert_eq!(kept["enabled"], json!(false));
    assert_eq!(kept["allowed_groups"], json!(["team-a"]));

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "ZERO",
            "grant_amount_usd": "1",
            "period_seconds": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], json!("plan_name_exists"));

    let (status, user) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/users",
        Some(json!({
            "username": "plan_user",
            "password": "password"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let user_id = user["id"].as_str().expect("user id").to_string();

    let (status, body) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/users/{user_id}"),
        Some(json!({
            "billing_plan_id": "missing-plan"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_billing_plan"));

    let (status, fetched) = json_request(
        &ctx,
        Method::GET,
        &format!("/api/dashboard/users/{user_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["billing_plan_id"], json!(null));
}

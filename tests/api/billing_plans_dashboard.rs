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
    admin_auth: String,
    state: monoize::app::AppState,
}

impl Drop for TestContext {
    fn drop(&mut self) {
        super::close_test_state(&self.state, None);
    }
}

async fn setup() -> TestContext {
    super::assert_test_cleanup_succeeded();
    let state = load_state_with_runtime(RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: monoize::node_config::NodeSettings::primary_default(),
    })
    .await
    .expect("state loads");
    let admin = state
        .user_store
        .create_user("admin_billing_plans", "password", UserRole::Admin, None)
        .await
        .expect("admin created");
    let session = state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("session created");
    TestContext {
        router: build_app(state.clone()),
        admin_auth: format!("Bearer {}", session.token),
        state,
    }
}

async fn json_request_auth(
    ctx: &TestContext,
    auth: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(AUTHORIZATION, auth);
    let body = if let Some(body) = body {
        builder = builder.header(CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = ctx
        .router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    (status, value)
}

async fn admin_request(
    ctx: &TestContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    json_request_auth(ctx, &ctx.admin_auth, method, path, body).await
}

async fn create_group(ctx: &TestContext, name: &str) -> String {
    let (status, group) = admin_request(
        ctx,
        Method::POST,
        "/api/dashboard/groups",
        Some(json!({ "name": name, "user_selectable": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    group["id"].as_str().expect("group id").to_string()
}

fn plan_body(name: &str, group_id: &str) -> Value {
    json!({
        "name": name,
        "description": "Sliding-window plan",
        "limit_5h_nano_usd": "1000",
        "limit_24h_nano_usd": "3000",
        "limit_7d_nano_usd": null,
        "limit_30d_nano_usd": null,
        "group_ids": [group_id],
        "multiplier": "0.5",
        "listed": true,
        "prices": [{ "price_usd": "2.5", "duration_seconds": 2592000 }]
    })
}

#[tokio::test]
async fn billing_plan_crud_validates_limits_groups_multiplier_and_prices() {
    let ctx = setup().await;
    let group_id = create_group(&ctx, "team-a").await;

    for (mut body, code) in [
        (plan_body("bad-limits", &group_id), "invalid_plan_limits"),
        (plan_body("bad-groups", &group_id), "invalid_plan_groups"),
        (
            plan_body("bad-multiplier", &group_id),
            "invalid_plan_multiplier",
        ),
        (plan_body("bad-prices", &group_id), "invalid_plan_prices"),
    ] {
        match code {
            "invalid_plan_limits" => {
                body["limit_5h_nano_usd"] = Value::Null;
                body["limit_24h_nano_usd"] = Value::Null;
            }
            "invalid_plan_groups" => body["group_ids"] = json!(["missing-group"]),
            "invalid_plan_multiplier" => body["multiplier"] = json!("0"),
            "invalid_plan_prices" => body["prices"] = json!([]),
            _ => unreachable!(),
        }
        let (status, response) = admin_request(
            &ctx,
            Method::POST,
            "/api/dashboard/billing-plans",
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["error"]["code"], json!(code));
    }

    let (status, created) = admin_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(plan_body("Starter", &group_id)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["prices"][0]["price_usd"], json!("2.5"));
    assert_eq!(created["multiplier"], json!("0.5"));
    let plan_id = created["id"].as_str().unwrap();

    let (status, duplicate) = admin_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(plan_body("starter", &group_id)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(duplicate["error"]["code"], json!("plan_name_exists"));

    let mut update = plan_body("Starter Plus", &group_id);
    update["listed"] = json!(false);
    update["prices"] = json!([]);
    let (status, _) = admin_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/billing-plans/{plan_id}"),
        Some(update),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, marketplace) = admin_request(
        &ctx,
        Method::GET,
        "/api/dashboard/billing-plans/marketplace",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(marketplace.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn listed_plan_purchase_uses_prepaid_and_returns_separate_subscription() {
    let ctx = setup().await;
    let group_id = create_group(&ctx, "purchase-group").await;
    let (status, plan) = admin_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(plan_body("Monthly", &group_id)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let plan_id = plan["id"].as_str().unwrap();
    let price_id = plan["prices"][0]["id"].as_str().unwrap();

    let user = ctx
        .state
        .user_store
        .create_user("plan_buyer", "password", UserRole::User, None)
        .await
        .unwrap();
    ctx.state
        .user_store
        .admin_adjust_user_balance(&user.id, Some("5000000000".to_string()), None, "admin")
        .await
        .unwrap();
    let session = ctx
        .state
        .user_store
        .create_session(&user.id, 7)
        .await
        .unwrap();
    let user_auth = format!("Bearer {}", session.token);

    let (status, purchased) = json_request_auth(
        &ctx,
        &user_auth,
        Method::POST,
        &format!("/api/dashboard/billing-plans/{plan_id}/purchase"),
        Some(json!({ "price_id": price_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(purchased["plan_name"], json!("Monthly"));
    assert_eq!(
        purchased["windows"]["five_hour"]["remaining_nano_usd"],
        json!("1000")
    );

    let (status, me) = json_request_auth(
        &ctx,
        &user_auth,
        Method::GET,
        "/api/dashboard/auth/me",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["balance_nano_usd"], json!("2500000000"));
    assert!(me.get("billing_plan").is_none());

    let (status, second) = json_request_auth(
        &ctx,
        &user_auth,
        Method::POST,
        &format!("/api/dashboard/billing-plans/{plan_id}/purchase"),
        Some(json!({ "price_id": price_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(second["error"]["code"], json!("active_subscription_exists"));

    let (status, ledger) = json_request_auth(
        &ctx,
        &user_auth,
        Method::GET,
        "/api/dashboard/ledger?limit=20&offset=0&kinds=plan_purchase",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ledger["entries"][0]["delta_nano_usd"], json!("-2500000000"));
}

#[tokio::test]
async fn eligible_charge_uses_multiplier_and_plan_before_prepaid_fallback() {
    let ctx = setup().await;
    let group_id = create_group(&ctx, "charge-group").await;
    let other_group_id = create_group(&ctx, "other-group").await;
    let (status, plan) = admin_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(plan_body("Charge Plan", &group_id)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let user = ctx
        .state
        .user_store
        .create_user("plan_charge", "password", UserRole::User, None)
        .await
        .unwrap();
    ctx.state
        .user_store
        .admin_adjust_user_balance(&user.id, Some("3000000000".to_string()), None, "admin")
        .await
        .unwrap();
    ctx.state
        .user_store
        .purchase_billing_plan(
            &user.id,
            plan["id"].as_str().unwrap(),
            plan["prices"][0]["id"].as_str().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let (key, _) = ctx
        .state
        .user_store
        .create_api_key(&user.id, "plan-key", None)
        .await
        .unwrap();

    let first = ctx
        .state
        .user_store
        .charge_user_balance_nano(
            &user.id,
            1200,
            &json!({ "request_id": "plan-request-1", "api_key_id": key.id.clone(), "billing_group_id": group_id.clone() }),
        )
        .await
        .unwrap();
    assert_eq!(first.adjusted_charge_nano_usd, 600);
    assert_eq!(first.plan_covered_nano_usd, 600);
    assert_eq!(first.fallback_nano_usd, 0);

    let second = ctx
        .state
        .user_store
        .charge_user_balance_nano(
            &user.id,
            1200,
            &json!({ "request_id": "plan-request-2", "api_key_id": key.id.clone(), "billing_group_id": group_id }),
        )
        .await
        .unwrap();
    assert_eq!(second.plan_covered_nano_usd, 400);
    assert_eq!(second.fallback_nano_usd, 200);

    let ineligible = ctx
        .state
        .user_store
        .charge_user_balance_nano(
            &user.id,
            100,
            &json!({ "request_id": "plan-request-3", "api_key_id": key.id, "billing_group_id": other_group_id }),
        )
        .await
        .unwrap();
    assert_eq!(ineligible.adjusted_charge_nano_usd, 100);
    assert_eq!(ineligible.plan_covered_nano_usd, 0);
    assert_eq!(ineligible.fallback_nano_usd, 100);
    assert_eq!(
        ctx.state
            .user_store
            .get_user_balance(&user.id)
            .await
            .unwrap()
            .unwrap()
            .balance_nano_usd,
        499999700
    );
}

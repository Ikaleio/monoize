use axum::Json;
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
    state: monoize::app::AppState,
}

impl Drop for TestContext {
    fn drop(&mut self) {
        super::close_test_state(&self.state, None);
    }
}

async fn setup() -> TestContext {
    super::assert_test_cleanup_succeeded();
    async fn siteverify(Json(body): Json<Value>) -> Json<Value> {
        Json(json!({
            "success": body["secret"] == json!("test-cap-secret")
                && body["response"] == json!("test-captcha-token")
        }))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test Cap server");
    let address = listener.local_addr().expect("test Cap address");
    tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route("/site-key/siteverify", axum::routing::post(siteverify)),
        )
        .await
        .expect("serve test Cap endpoint");
    });

    let mut state = load_state_with_runtime(RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: monoize::node_config::NodeSettings::primary_default(),
    })
    .await
    .expect("state loads");
    state.cap_verifier = monoize::captcha::CapVerifier::configured(
        &format!("http://{address}/site-key/"),
        "test-cap-secret".to_string(),
    )
    .expect("configure test Cap verifier");
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
        auth_header: format!("Bearer {}", session.token),
        state,
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

async fn create_group(ctx: &TestContext, name: &str) -> String {
    let (status, group) = json_request(
        ctx,
        Method::POST,
        "/api/dashboard/groups",
        Some(json!({ "name": name, "user_selectable": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    group["id"].as_str().expect("group id").to_string()
}

#[tokio::test]
async fn billing_plan_validation_and_assignment_error_codes() {
    let ctx = setup().await;
    let team_a_group_id = create_group(&ctx, "team-a").await;

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "bad-period",
            "grant_amount_usd": "1",
            "schedule": "bad"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_schedule"));

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "bad-amount",
            "grant_amount_nano_usd": "abc",
            "schedule": "* * * * *"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_grant_amount"));

    // GR-C3: plan ceilings must reference registered group ids.
    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "bad-group",
            "grant_amount_usd": "1",
            "schedule": "* * * * *",
            "group_ids": ["missing-group"]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("invalid_request"));

    let (status, created) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "zero",
            "grant_amount_usd": "0",
            "schedule": "* * * * *",
            "group_ids": [team_a_group_id],
            "enabled": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["grant_amount_nano_usd"], json!("0"));
    assert_eq!(created["enabled"], json!(false));
    assert_eq!(created["group_ids"], json!([team_a_group_id]));
    let plan_id = created["id"].as_str().expect("plan id").to_string();

    let (status, _) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/billing-plans/{plan_id}"),
        Some(json!({
            "name": "zero",
            "grant_amount_nano_usd": "0",
            "schedule": "* * * * *"
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
    assert_eq!(kept["group_ids"], json!([team_a_group_id]));

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "ZERO",
            "grant_amount_usd": "1",
            "schedule": "* * * * *"
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
    assert_eq!(fetched["billing_plan"], json!(null));
}

#[tokio::test]
async fn assigned_plan_is_embedded_on_user_and_me_payloads() {
    let ctx = setup().await;
    let team_a_group_id = create_group(&ctx, "team-a").await;

    let (status, plan) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "Starter",
            "grant_amount_usd": "10",
            "schedule": "0 0 * * *",
            "group_ids": [team_a_group_id]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let plan_id = plan["id"].as_str().expect("plan id").to_string();

    let (status, user) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/users",
        Some(json!({
            "username": "subscriber",
            "password": "password"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let user_id = user["id"].as_str().expect("user id").to_string();
    assert_eq!(user["billing_plan"], json!(null));
    assert!(user.get("today_calls").is_none());

    let (status, assigned) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/users/{user_id}"),
        Some(json!({ "billing_plan_id": plan_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assigned["billing_plan_id"], json!(plan_id));
    assert_eq!(assigned["balance_usd"], json!("10"));
    assert_eq!(assigned["balance_nano_usd"], json!("10000000000"));
    assert_eq!(assigned["billing_plan"]["name"], json!("Starter"));
    assert_eq!(assigned["billing_plan"]["grant_amount_usd"], json!("10"));
    assert_eq!(assigned["billing_plan"]["schedule"], json!("0 0 * * *"));
    assert_eq!(
        assigned["billing_plan"]["group_ids"],
        json!([team_a_group_id])
    );
    assert_eq!(assigned["billing_plan"]["enabled"], json!(true));
    assert!(assigned.get("today_calls").is_none());

    let (status, listed) = json_request(&ctx, Method::GET, "/api/dashboard/users", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed_user = listed
        .as_array()
        .expect("users array")
        .iter()
        .find(|row| row["id"] == json!(user_id))
        .expect("subscriber listed");
    assert_eq!(listed_user["billing_plan"]["name"], json!("Starter"));
    assert_eq!(listed_user["today_calls"], json!(0));
    assert_eq!(listed_user["today_cost_nano_usd"], json!("0"));
    assert_eq!(listed_user["today_cost_usd"], json!("0"));

    let (status, login) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/auth/login",
        Some(json!({
            "username": "subscriber",
            "password": "password",
            "captcha_token": "test-captcha-token"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login["user"]["billing_plan"]["name"], json!("Starter"));
    assert!(login["user"].get("today_calls").is_none());

    let token = login["token"].as_str().expect("session token");
    let resp = ctx
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/dashboard/auth/me")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let me: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(me["billing_plan"]["id"], json!(plan_id));
    assert_eq!(me["billing_plan"]["name"], json!("Starter"));
    assert!(me.get("today_calls").is_none());
}

#[tokio::test]
async fn reset_plan_refills_eligible_subscribers_only() {
    let ctx = setup().await;

    let (status, body) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans/missing-plan/reset",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], json!("not_found"));

    let (status, plan) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "Resettable",
            "grant_amount_usd": "8",
            "schedule": "0 0 * * *"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let plan_id = plan["id"].as_str().expect("plan id").to_string();

    let (status, other_plan) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/billing-plans",
        Some(json!({
            "name": "Other",
            "grant_amount_usd": "3",
            "schedule": "0 0 * * *"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_plan_id = other_plan["id"]
        .as_str()
        .expect("other plan id")
        .to_string();

    let (status, subscriber) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/users",
        Some(json!({
            "username": "reset_sub",
            "password": "password"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let subscriber_id = subscriber["id"].as_str().expect("user id").to_string();

    let (status, outsider) = json_request(
        &ctx,
        Method::POST,
        "/api/dashboard/users",
        Some(json!({
            "username": "reset_out",
            "password": "password"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let outsider_id = outsider["id"].as_str().expect("user id").to_string();

    let (status, assigned) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/users/{subscriber_id}"),
        Some(json!({ "billing_plan_id": plan_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assigned["balance_usd"], json!("8"));

    let (status, _) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/users/{outsider_id}"),
        Some(json!({ "billing_plan_id": other_plan_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, spent) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/users/{subscriber_id}"),
        Some(json!({ "balance_usd": "1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(spent["balance_usd"], json!("1"));

    let (status, spent_out) = json_request(
        &ctx,
        Method::PUT,
        &format!("/api/dashboard/users/{outsider_id}"),
        Some(json!({ "balance_usd": "1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(spent_out["balance_usd"], json!("1"));

    let (status, reset) = json_request(
        &ctx,
        Method::POST,
        &format!("/api/dashboard/billing-plans/{plan_id}/reset"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reset["success"], json!(true));
    assert_eq!(reset["reset_count"], json!(1));

    let (status, refilled) = json_request(
        &ctx,
        Method::GET,
        &format!("/api/dashboard/users/{subscriber_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refilled["balance_usd"], json!("8"));
    assert_eq!(refilled["balance_nano_usd"], json!("8000000000"));

    let (status, untouched) = json_request(
        &ctx,
        Method::GET,
        &format!("/api/dashboard/users/{outsider_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(untouched["balance_usd"], json!("1"));
}

//! Integration tests for `recharge-system.spec.md` §15 (T2, T5..T12).
//! T1 (conversion), T3 (EPay sign round-trip), and T4 (Stripe signature)
//! are unit tests inside `src/recharge/`.

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use md5::Digest;
use monoize::app::{AppState, RuntimeConfig, build_app, load_state_with_runtime};
use monoize::recharge::store::RechargeOrder;
use monoize::recharge::{NotifyResult, VerifiedNotification};
use monoize::users::UserRole;
use serde_json::{Value, json};
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

async fn request(
    router: &axum::Router,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
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

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).expect("body is JSON")
}

async fn error_code(response: axum::response::Response) -> String {
    body_json(response).await["error"]["code"]
        .as_str()
        .expect("error code present")
        .to_string()
}

struct Ctx {
    state: AppState,
    router: axum::Router,
    admin_token: String,
    admin_id: String,
    user_token: String,
    user_id: String,
}

async fn setup() -> Ctx {
    let state = test_state().await;
    let router = build_app(state.clone());
    let admin = state
        .user_store
        .create_user("recharge_admin", "password123", UserRole::Admin, None)
        .await
        .expect("admin creates");
    let admin_token = state
        .user_store
        .create_session(&admin.id, 7)
        .await
        .expect("admin session")
        .token;
    let user = state
        .user_store
        .create_user("recharge_user", "password123", UserRole::User, None)
        .await
        .expect("user creates");
    let user_token = state
        .user_store
        .create_session(&user.id, 7)
        .await
        .expect("user session")
        .token;
    Ctx {
        state,
        router,
        admin_token,
        admin_id: admin.id,
        user_token,
        user_id: user.id,
    }
}

async fn set_origin(ctx: &Ctx) {
    let response = request(
        &ctx.router,
        Method::PUT,
        "/api/dashboard/settings",
        Some(&ctx.admin_token),
        Some(json!({ "recharge_public_origin": "https://pay.example.com" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn create_epay_channel(ctx: &Ctx, name: &str, merchant_key: &str) -> String {
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/payment-channels",
        Some(&ctx.admin_token),
        Some(json!({
            "name": name,
            "type_id": "epay",
            "currency": "CNY",
            "usd_rate": "7.30",
            "config": {
                "gateway_url": "https://epay.example.com",
                "merchant_id": "1001",
                "merchant_key": merchant_key,
            },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["channel"]["id"]
        .as_str()
        .expect("channel id")
        .to_string()
}

async fn create_order(ctx: &Ctx, channel_id: &str, credit_usd: &str) -> Value {
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/recharge/orders",
        Some(&ctx.user_token),
        Some(json!({ "payment_channel_id": channel_id, "credit_usd": credit_usd })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

fn epay_sign(pairs: &[(&str, String)], merchant_key: &str) -> String {
    let mut signable: Vec<&(&str, String)> = pairs
        .iter()
        .filter(|(key, value)| *key != "sign" && *key != "sign_type" && !value.is_empty())
        .collect();
    signable.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let joined = signable
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut hasher = md5::Md5::new();
    hasher.update(joined.as_bytes());
    hasher.update(merchant_key.as_bytes());
    hex::encode(hasher.finalize())
}

fn epay_success_query(order_id: &str, money: &str, merchant_key: &str) -> String {
    let mut pairs = vec![
        ("pid", "1001".to_string()),
        ("out_trade_no", order_id.to_string()),
        ("trade_no", format!("prov-{order_id}")),
        ("trade_status", "TRADE_SUCCESS".to_string()),
        ("money", money.to_string()),
    ];
    let sign = epay_sign(&pairs, merchant_key);
    pairs.push(("sign", sign));
    pairs.push(("sign_type", "MD5".to_string()));
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

async fn notify(ctx: &Ctx, channel_id: &str, query: &str) -> axum::response::Response {
    request(
        &ctx.router,
        Method::GET,
        &format!("/api/pay/notify/{channel_id}?{query}"),
        None,
        None,
    )
    .await
}

async fn balance_nano(ctx: &Ctx, user_id: &str) -> i128 {
    ctx.state
        .user_store
        .get_user_balance(user_id)
        .await
        .expect("balance reads")
        .expect("user exists")
        .balance_nano_usd
}

/// Spec §15 T2: every RC-O3 error code, including the pending cap.
#[tokio::test]
async fn order_creation_validation_codes() {
    let ctx = setup().await;
    let channel_id = create_epay_channel(&ctx, "epay-t2", "key-t2").await;

    // 1. Unknown channel.
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/recharge/orders",
        Some(&ctx.user_token),
        Some(json!({ "payment_channel_id": "missing", "credit_usd": "10" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(response).await, "not_found");

    // 2. Disabled channel (checked before the origin).
    let response = request(
        &ctx.router,
        Method::PUT,
        &format!("/api/dashboard/payment-channels/{channel_id}"),
        Some(&ctx.admin_token),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/recharge/orders",
        Some(&ctx.user_token),
        Some(json!({ "payment_channel_id": channel_id, "credit_usd": "10" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "channel_disabled");
    let response = request(
        &ctx.router,
        Method::PUT,
        &format!("/api/dashboard/payment-channels/{channel_id}"),
        Some(&ctx.admin_token),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // 3. Origin unset (RC-G3).
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/recharge/orders",
        Some(&ctx.user_token),
        Some(json!({ "payment_channel_id": channel_id, "credit_usd": "10" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "recharge_origin_unset");
    set_origin(&ctx).await;

    // 4. Invalid amount.
    for body in [
        json!({ "payment_channel_id": channel_id }),
        json!({ "payment_channel_id": channel_id, "credit_usd": "0" }),
        json!({ "payment_channel_id": channel_id, "credit_usd": "-5" }),
        json!({ "payment_channel_id": channel_id, "credit_nano_usd": "01" }),
    ] {
        let response = request(
            &ctx.router,
            Method::POST,
            "/api/dashboard/recharge/orders",
            Some(&ctx.user_token),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error_code(response).await, "invalid_amount");
    }

    // 5. Out of range (default bounds are [1, 10000] USD).
    for credit in ["0.5", "10001"] {
        let response = request(
            &ctx.router,
            Method::POST,
            "/api/dashboard/recharge/orders",
            Some(&ctx.user_token),
            Some(json!({ "payment_channel_id": channel_id, "credit_usd": credit })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error_code(response).await, "amount_out_of_range");
    }

    // 6. Pending cap (default 10).
    for _ in 0..10 {
        create_order(&ctx, &channel_id, "10").await;
    }
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/recharge/orders",
        Some(&ctx.user_token),
        Some(json!({ "payment_channel_id": channel_id, "credit_usd": "10" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_code(response).await, "too_many_pending_orders");
}

#[tokio::test]
async fn concurrent_order_creation_cannot_exceed_pending_cap() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-pending-race", "key-pending-race").await;
    let futures = (0..24).map(|_| {
        request(
            &ctx.router,
            Method::POST,
            "/api/dashboard/recharge/orders",
            Some(&ctx.user_token),
            Some(json!({ "payment_channel_id": channel_id, "credit_usd": "10" })),
        )
    });
    let responses = futures_util::future::join_all(futures).await;
    let successes = responses
        .iter()
        .filter(|response| response.status() == StatusCode::OK)
        .count();
    let rejected = responses
        .iter()
        .filter(|response| response.status() == StatusCode::TOO_MANY_REQUESTS)
        .count();
    assert_eq!(successes, 10);
    assert_eq!(rejected, 14);
}

#[tokio::test]
async fn successful_notification_without_paid_fields_fails_closed() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-missing-paid", "key-missing-paid").await;
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"]
        .as_str()
        .expect("order id")
        .to_string();
    let result = ctx
        .state
        .user_store
        .apply_verified_notification(
            &channel_id,
            &VerifiedNotification {
                order_id: order_id.clone(),
                provider_order_id: Some("provider-missing-paid".to_string()),
                result: NotifyResult::Success,
                paid_amount: None,
                paid_currency: None,
            },
        )
        .await;
    assert!(result.is_err());
    let order = ctx
        .state
        .user_store
        .get_recharge_order(&order_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(order.status, "pending");
    assert_eq!(balance_nano(&ctx, &ctx.user_id).await, 0);
}

/// Spec §15 T5: two sequential success notifications credit exactly once.
#[tokio::test]
async fn sequential_notifications_credit_exactly_once() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t5", "key-t5").await;
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"].as_str().expect("order id");
    assert_eq!(created["order"]["pay_amount"], "73.00");
    assert_eq!(created["payment"]["kind"], "redirect");

    let before = balance_nano(&ctx, &ctx.user_id).await;
    let query = epay_success_query(order_id, "73.00", "key-t5");

    let first = notify(&ctx, &channel_id, &query).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(body_text(first).await, "success");
    let second = notify(&ctx, &channel_id, &query).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_text(second).await, "success");

    assert_eq!(
        balance_nano(&ctx, &ctx.user_id).await,
        before + 10_000_000_000
    );
    let order = ctx
        .state
        .user_store
        .get_recharge_order(order_id)
        .await
        .expect("order reads")
        .expect("order exists");
    assert_eq!(order.status, "succeeded");
    assert!(order.paid_at.is_some());

    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/ledger?kinds=recharge",
        Some(&ctx.user_token),
        None,
    )
    .await;
    let ledger = body_json(response).await;
    assert_eq!(ledger["total"], 1);
    assert_eq!(ledger["entries"][0]["delta_nano_usd"], "10000000000");
    assert_eq!(ledger["entries"][0]["meta_json"]["order_id"], *order_id);
}

/// Spec §15 T6: concurrent success notifications serialize to one credit, and
/// a pre-existing idempotency key aborts the credit transaction (RC-N8).
#[tokio::test]
async fn concurrent_notifications_and_idempotency_barrier() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t6", "key-t6").await;
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"]
        .as_str()
        .expect("order id")
        .to_string();

    let before = balance_nano(&ctx, &ctx.user_id).await;
    let verified = monoize::recharge::VerifiedNotification {
        order_id: order_id.clone(),
        provider_order_id: Some("prov-t6".to_string()),
        result: monoize::recharge::NotifyResult::Success,
        paid_amount: Some("73.00".to_string()),
        paid_currency: Some("CNY".to_string()),
    };
    let task = |store: monoize::users::UserStore,
                channel: String,
                verified: monoize::recharge::VerifiedNotification| {
        tokio::spawn(async move { store.apply_verified_notification(&channel, &verified).await })
    };
    let (left, right) = tokio::join!(
        task(
            ctx.state.user_store.clone(),
            channel_id.clone(),
            verified.clone()
        ),
        task(
            ctx.state.user_store.clone(),
            channel_id.clone(),
            verified.clone()
        ),
    );
    let outcomes = [
        left.expect("join").expect("apply"),
        right.expect("join").expect("apply"),
    ];
    use monoize::recharge::NotifyOutcome;
    assert!(outcomes.contains(&NotifyOutcome::Credited));
    assert!(outcomes.contains(&NotifyOutcome::Duplicate));
    assert_eq!(
        balance_nano(&ctx, &ctx.user_id).await,
        before + 10_000_000_000
    );

    // RC-N8 second barrier: a duplicated idempotency key rolls back the
    // whole transaction and leaves the order in its prior state.
    let created = create_order(&ctx, &channel_id, "10").await;
    let second_order_id = created["order"]["id"]
        .as_str()
        .expect("order id")
        .to_string();
    {
        use sea_orm::ConnectionTrait;
        let write = ctx.state.db_pool.write().await;
        write
            .execute(ctx.state.db_pool.stmt(
                "INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, \
                 balance_after_nano_usd, meta_json, created_at, idempotency_key) \
                 VALUES ($1, $2, 'recharge', '0', NULL, '{}', $3, $4)",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    ctx.user_id.clone().into(),
                    chrono::Utc::now().to_rfc3339().into(),
                    format!("recharge:{second_order_id}").into(),
                ],
            ))
            .await
            .expect("direct ledger insert");
    }
    let verified = monoize::recharge::VerifiedNotification {
        order_id: second_order_id.clone(),
        provider_order_id: None,
        result: monoize::recharge::NotifyResult::Success,
        paid_amount: Some("73.00".to_string()),
        paid_currency: Some("CNY".to_string()),
    };
    let outcome = ctx
        .state
        .user_store
        .apply_verified_notification(&channel_id, &verified)
        .await
        .expect("apply");
    assert_eq!(outcome, NotifyOutcome::Duplicate);
    let order = ctx
        .state
        .user_store
        .get_recharge_order(&second_order_id)
        .await
        .expect("order reads")
        .expect("order exists");
    assert_eq!(order.status, "pending");
    assert_eq!(
        balance_nano(&ctx, &ctx.user_id).await,
        before + 10_000_000_000
    );
}

/// Spec §15 T7: an amount mismatch fails the order and credits nothing.
#[tokio::test]
async fn amount_mismatch_fails_the_order() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t7", "key-t7").await;
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"].as_str().expect("order id");

    let before = balance_nano(&ctx, &ctx.user_id).await;
    let query = epay_success_query(order_id, "1.00", "key-t7");
    let response = notify(&ctx, &channel_id, &query).await;
    // failed_recorded ack stops gateway retries.
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "success");

    let order = ctx
        .state
        .user_store
        .get_recharge_order(order_id)
        .await
        .expect("order reads")
        .expect("order exists");
    assert_eq!(order.status, "failed");
    assert_eq!(order.error_code.as_deref(), Some("amount_mismatch"));
    assert_eq!(order.meta_json["mismatch"]["paid_amount"], "1.00");
    assert_eq!(balance_nano(&ctx, &ctx.user_id).await, before);
}

/// Spec §15 T8: the sweeper expires stale pending orders; a later verified
/// success on the expired order still credits exactly once (RC-X3).
#[tokio::test]
async fn expiry_and_late_credit() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t8", "key-t8").await;

    let now = chrono::Utc::now();
    let order_id = uuid::Uuid::new_v4().simple().to_string();
    let stale = RechargeOrder {
        id: order_id.clone(),
        user_id: ctx.user_id.clone(),
        payment_channel_id: channel_id.clone(),
        channel_type_id: "epay".to_string(),
        channel_name: "epay-t8".to_string(),
        status: "pending".to_string(),
        credit_nano_usd: 10_000_000_000,
        pay_currency: "CNY".to_string(),
        pay_amount: "73.00".to_string(),
        usd_rate: "7.30".to_string(),
        provider_order_id: None,
        error_code: None,
        paid_at: None,
        expires_at: (now - chrono::Duration::seconds(30)).to_rfc3339(),
        meta_json: Value::Object(Default::default()),
        created_at: (now - chrono::Duration::seconds(3700)).to_rfc3339(),
        updated_at: (now - chrono::Duration::seconds(3700)).to_rfc3339(),
        username: None,
    };
    ctx.state
        .user_store
        .insert_recharge_order(&stale)
        .await
        .expect("stale order inserts");

    let before = balance_nano(&ctx, &ctx.user_id).await;
    let expired = ctx
        .state
        .user_store
        .expire_due_recharge_orders()
        .await
        .expect("sweep runs");
    assert_eq!(expired, 1);
    let order = ctx
        .state
        .user_store
        .get_recharge_order(&order_id)
        .await
        .expect("order reads")
        .expect("order exists");
    assert_eq!(order.status, "expired");
    // RC-X2: expiry writes no ledger row and mutates no balance.
    assert_eq!(balance_nano(&ctx, &ctx.user_id).await, before);

    let query = epay_success_query(&order_id, "73.00", "key-t8");
    let response = notify(&ctx, &channel_id, &query).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "success");
    let order = ctx
        .state
        .user_store
        .get_recharge_order(&order_id)
        .await
        .expect("order reads")
        .expect("order exists");
    assert_eq!(order.status, "succeeded");
    assert_eq!(
        balance_nano(&ctx, &ctx.user_id).await,
        before + 10_000_000_000
    );
}

/// Spec §15 T9: manual refund debits into a possibly negative balance; a
/// second refund attempt returns `invalid_order_state`.
#[tokio::test]
async fn refund_flow_and_double_refund_rejection() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t9", "key-t9").await;
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"]
        .as_str()
        .expect("order id")
        .to_string();
    let query = epay_success_query(&order_id, "73.00", "key-t9");
    let response = notify(&ctx, &channel_id, &query).await;
    assert_eq!(body_text(response).await, "success");

    // Drain the wallet so the refund produces debt (B6: representable).
    let response = request(
        &ctx.router,
        Method::PUT,
        &format!("/api/dashboard/users/{}", ctx.user_id),
        Some(&ctx.admin_token),
        Some(json!({ "balance_usd": "0" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // RC-R4: epay requires the manual acknowledgment.
    let response = request(
        &ctx.router,
        Method::POST,
        &format!("/api/dashboard/recharge/orders/{order_id}/refund"),
        Some(&ctx.admin_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(response).await, "manual_refund_required");

    // Non-admin callers are rejected before any state change.
    let response = request(
        &ctx.router,
        Method::POST,
        &format!("/api/dashboard/recharge/orders/{order_id}/refund"),
        Some(&ctx.user_token),
        Some(json!({ "manual": true })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = request(
        &ctx.router,
        Method::POST,
        &format!("/api/dashboard/recharge/orders/{order_id}/refund"),
        Some(&ctx.admin_token),
        Some(json!({ "manual": true })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let refunded = body_json(response).await;
    assert_eq!(refunded["order"]["status"], "refunded");
    assert_eq!(balance_nano(&ctx, &ctx.user_id).await, -10_000_000_000);

    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/ledger?kinds=recharge_refund",
        Some(&ctx.admin_token),
        None,
    )
    .await;
    let ledger = body_json(response).await;
    assert_eq!(ledger["total"], 1);
    assert_eq!(ledger["entries"][0]["delta_nano_usd"], "-10000000000");
    assert_eq!(ledger["entries"][0]["meta_json"]["manual"], true);
    assert_eq!(
        ledger["entries"][0]["meta_json"]["actor_user_id"],
        *ctx.admin_id
    );

    let response = request(
        &ctx.router,
        Method::POST,
        &format!("/api/dashboard/recharge/orders/{order_id}/refund"),
        Some(&ctx.admin_token),
        Some(json!({ "manual": true })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(response).await, "invalid_order_state");
}

/// Spec §15 T10: role `user` sees only own orders and ledger entries; the
/// `username` filter is ignored for role `user` and honored for admins.
#[tokio::test]
async fn role_scoping_on_orders_and_ledger() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t10", "key-t10").await;
    // One order owned by recharge_user.
    create_order(&ctx, &channel_id, "10").await;
    // One order owned by another user.
    let other = ctx
        .state
        .user_store
        .create_user("recharge_other", "password123", UserRole::User, None)
        .await
        .expect("other user creates");
    let other_token = ctx
        .state
        .user_store
        .create_session(&other.id, 7)
        .await
        .expect("other session")
        .token;
    let response = request(
        &ctx.router,
        Method::POST,
        "/api/dashboard/recharge/orders",
        Some(&other_token),
        Some(json!({ "payment_channel_id": channel_id, "credit_usd": "20" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let other_order = body_json(response).await["order"]["id"]
        .as_str()
        .expect("order id")
        .to_string();

    // Role user: own rows only; username filter ignored; no username field.
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/recharge/orders?username=recharge_other",
        Some(&ctx.user_token),
        None,
    )
    .await;
    let orders = body_json(response).await;
    assert_eq!(orders["total"], 1);
    assert_eq!(orders["orders"][0]["user_id"], *ctx.user_id);
    assert!(orders["orders"][0].get("username").is_none());

    // Role user: another user's order is a 404 (RC-A4).
    let response = request(
        &ctx.router,
        Method::GET,
        &format!("/api/dashboard/recharge/orders/{other_order}"),
        Some(&ctx.user_token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Admin: all rows; username filter honored; username field present.
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/recharge/orders",
        Some(&ctx.admin_token),
        None,
    )
    .await;
    assert_eq!(body_json(response).await["total"], 2);
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/recharge/orders?username=recharge_other",
        Some(&ctx.admin_token),
        None,
    )
    .await;
    let orders = body_json(response).await;
    assert_eq!(orders["total"], 1);
    assert_eq!(orders["orders"][0]["username"], "recharge_other");

    // Ledger scoping: credit both orders, then compare visibility.
    let user_order = {
        let response = request(
            &ctx.router,
            Method::GET,
            "/api/dashboard/recharge/orders",
            Some(&ctx.user_token),
            None,
        )
        .await;
        body_json(response).await["orders"][0]["id"]
            .as_str()
            .expect("order id")
            .to_string()
    };
    for (order_id, money) in [(&user_order, "73.00"), (&other_order, "146.00")] {
        let query = epay_success_query(order_id, money, "key-t10");
        let response = notify(&ctx, &channel_id, &query).await;
        assert_eq!(body_text(response).await, "success");
    }
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/ledger?kinds=recharge",
        Some(&ctx.user_token),
        None,
    )
    .await;
    let ledger = body_json(response).await;
    assert_eq!(ledger["total"], 1);
    assert_eq!(ledger["entries"][0]["user_id"], *ctx.user_id);
    assert!(ledger["entries"][0].get("username").is_none());
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/ledger?kinds=recharge&username=recharge_other",
        Some(&ctx.admin_token),
        None,
    )
    .await;
    let ledger = body_json(response).await;
    assert_eq!(ledger["total"], 1);
    assert_eq!(ledger["entries"][0]["username"], "recharge_other");

    // RC-A5: a malformed kinds entry is rejected.
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/ledger?kinds=Recharge!",
        Some(&ctx.user_token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(response).await, "invalid_request");
}

/// Spec §15 T11: secret masking on read, keep-on-empty and replace-on-value
/// semantics on update (RC-P6).
#[tokio::test]
async fn channel_secret_masking_and_update_semantics() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t11", "key-original").await;

    // Read masks the secret; non-secret fields stay visible.
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/payment-channels",
        Some(&ctx.admin_token),
        None,
    )
    .await;
    let channels = body_json(response).await;
    assert_eq!(channels["channels"][0]["config"]["merchant_key"], "");
    assert_eq!(channels["channels"][0]["config"]["merchant_id"], "1001");
    // RC-A1 exposes no config in any form.
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/recharge/channels",
        Some(&ctx.user_token),
        None,
    )
    .await;
    let channels = body_json(response).await;
    assert!(channels["channels"][0].get("config").is_none());
    assert!(channels["channels"][0].get("config_json").is_none());
    assert_eq!(channels["channels"][0]["pay_scale"], 2);

    // PUT with an empty secret keeps the stored key: a notification signed
    // with the original key still verifies.
    let response = request(
        &ctx.router,
        Method::PUT,
        &format!("/api/dashboard/payment-channels/{channel_id}"),
        Some(&ctx.admin_token),
        Some(json!({
            "config": {
                "gateway_url": "https://epay.example.com",
                "merchant_id": "1001",
                "merchant_key": "",
            }
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"].as_str().expect("order id");
    let query = epay_success_query(order_id, "73.00", "key-original");
    let response = notify(&ctx, &channel_id, &query).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "success");

    // PUT with a non-empty secret replaces it: the old key now fails and the
    // new key verifies.
    let response = request(
        &ctx.router,
        Method::PUT,
        &format!("/api/dashboard/payment-channels/{channel_id}"),
        Some(&ctx.admin_token),
        Some(json!({
            "config": {
                "gateway_url": "https://epay.example.com",
                "merchant_id": "1001",
                "merchant_key": "key-replaced",
            }
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"].as_str().expect("order id");
    let old_key_query = epay_success_query(order_id, "73.00", "key-original");
    let response = notify(&ctx, &channel_id, &old_key_query).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let new_key_query = epay_success_query(order_id, "73.00", "key-replaced");
    let response = notify(&ctx, &channel_id, &new_key_query).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "success");

    // Channel CRUD requires an admin session.
    let response = request(
        &ctx.router,
        Method::GET,
        "/api/dashboard/payment-channels",
        Some(&ctx.user_token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Spec §15 T12: notify surface — unknown channel, invalid signature, and
/// unknown order each leave every order untouched.
#[tokio::test]
async fn notify_surface_edge_cases() {
    let ctx = setup().await;
    set_origin(&ctx).await;
    let channel_id = create_epay_channel(&ctx, "epay-t12", "key-t12").await;
    let created = create_order(&ctx, &channel_id, "10").await;
    let order_id = created["order"]["id"].as_str().expect("order id");

    // Unknown channel id → 404 with an empty body.
    let query = epay_success_query(order_id, "73.00", "key-t12");
    let response = notify(&ctx, "missing-channel", &query).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_text(response).await, "");

    // Invalid signature → adapter signature_error ack, no state change.
    let bad_query = epay_success_query(order_id, "73.00", "key-wrong");
    let response = notify(&ctx, &channel_id, &bad_query).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_text(response).await, "fail");

    // Unknown order → unknown_order ack, no state change.
    let unknown_query = epay_success_query(&"b".repeat(32), "73.00", "key-t12");
    let response = notify(&ctx, &channel_id, &unknown_query).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "fail");

    let order = ctx
        .state
        .user_store
        .get_recharge_order(order_id)
        .await
        .expect("order reads")
        .expect("order exists");
    assert_eq!(order.status, "pending");
}

/// RC-G1: settings validation for `recharge_public_origin`.
#[tokio::test]
async fn recharge_origin_validation() {
    let ctx = setup().await;
    for origin in [
        "example.com",
        "https://example.com/",
        "https://example.com/path",
        "https://example.com?q=1",
        "ftp://example.com",
    ] {
        let response = request(
            &ctx.router,
            Method::PUT,
            "/api/dashboard/settings",
            Some(&ctx.admin_token),
            Some(json!({ "recharge_public_origin": origin })),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "accepted {origin:?}"
        );
        assert_eq!(error_code(response).await, "invalid_request");
    }
    let response = request(
        &ctx.router,
        Method::PUT,
        "/api/dashboard/settings",
        Some(&ctx.admin_token),
        Some(json!({ "recharge_public_origin": "https://pay.example.com:8443" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await["recharge_public_origin"],
        "https://pay.example.com:8443"
    );
}

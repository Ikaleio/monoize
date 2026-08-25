//! `primary-replica-deployment.spec.md` test matrix (T1–T8).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::routing::post;
use http_body_util::BodyExt;
use monoize::db_cache::{LastUsedBatcher, RequestLogBatcher};
use monoize::node_config::NodeRole;
use monoize::replica::metering::{
    BalanceDelta, DeltaSpool, HEARTBEAT_EVICT_INTERVALS, REPLICA_IDENTITY_FILE_NAME,
    ReplicaHeartbeat, ReplicaHeartbeatRecord, ReplicaMetering, ShipTick, apply_metering_batch,
    drain_delta_spool_to_local_db, evict_expired_heartbeats, resolve_replica_identity,
};
use sea_orm::ConnectionTrait;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;

fn test_runtime(database_dsn: String) -> monoize::app::RuntimeConfig {
    monoize::app::RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn,
        request_log_spool_dir: None,
        node: monoize::node_config::NodeSettings::primary_default(),
    }
}

fn delta(kind: &str, user_id: &str, api_key_id: Option<&str>, amount: i128) -> BalanceDelta {
    BalanceDelta {
        delta_id: uuid::Uuid::new_v4().to_string(),
        kind: kind.to_string(),
        user_id: user_id.to_string(),
        api_key_id: api_key_id.map(str::to_string),
        amount_nano_usd: amount.to_string(),
        meta_json: serde_json::json!({ "request_id": "req-1" }),
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

async fn boot() -> (TempDir, monoize::app::AppState) {
    let temp = TempDir::new().unwrap();
    let dsn = format!("sqlite://{}", temp.path().join("m.db").display());
    let state = monoize::app::load_state_with_runtime(test_runtime(dsn))
        .await
        .expect("state loads");
    (temp, state)
}

#[tokio::test]
async fn ingest_applies_balance_delta_idempotently() {
    let (_temp, state) = boot().await;
    let user = state
        .user_store
        .create_user("delta_user", "pw", monoize::users::UserRole::User, None)
        .await
        .expect("user");
    state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("5000000000"),
            None,
            None,
            None,
        )
        .await
        .expect("seed balance");

    let batch = monoize::replica::metering::MeteringBatch {
        replica: None,
        request_logs: vec![],
        last_used: vec![],
        balance_deltas: vec![delta("request_charge", &user.id, None, 1_000_000_000)],
    };

    // T2 first delivery: one ledger row, balance reduced.
    let ack1 = apply_metering_batch(&state.db_pool, &batch)
        .await
        .expect("apply");
    assert_eq!(ack1.applied_balance_deltas, 1);
    let balance = state
        .user_store
        .get_user_balance(&user.id)
        .await
        .expect("balance")
        .expect("row");
    assert_eq!(balance.balance_nano_usd, 4_000_000_000);

    // I6 replay: nothing changes, counts report zero new applies.
    let ack2 = apply_metering_batch(&state.db_pool, &batch)
        .await
        .expect("replay");
    assert_eq!(ack2.applied_balance_deltas, 0);
    assert_eq!(ack2.applied_request_logs, 0);
    let balance2 = state
        .user_store
        .get_user_balance(&user.id)
        .await
        .expect("balance")
        .unwrap();
    assert_eq!(balance2.balance_nano_usd, 4_000_000_000);
}

#[tokio::test]
async fn ingest_allows_negative_result_and_counts_unlimited_as_applied_without_update() {
    let (_temp, state) = boot().await;
    let limited = state
        .user_store
        .create_user("limited", "pw", monoize::users::UserRole::User, None)
        .await
        .expect("limited");
    let unlimited = state
        .user_store
        .create_user("unl", "pw", monoize::users::UserRole::User, None)
        .await
        .expect("unlimited");
    state
        .user_store
        .update_user(
            &unlimited.id,
            None,
            None,
            None,
            None,
            Some("0"),
            Some(true),
            None,
            None,
        )
        .await
        .expect("make unlimited");

    // T3 negative result allowed on the limited user.
    let batch = monoize::replica::metering::MeteringBatch {
        replica: None,
        request_logs: vec![],
        last_used: vec![],
        balance_deltas: vec![delta("request_charge", &limited.id, None, 100)],
    };
    let ack = apply_metering_batch(&state.db_pool, &batch)
        .await
        .expect("apply");
    assert_eq!(ack.applied_balance_deltas, 1);
    let bal = state
        .user_store
        .get_user_balance(&limited.id)
        .await
        .expect("bal")
        .unwrap();
    assert_eq!(bal.balance_nano_usd, -100);

    // T3 unlimited owner: ledger event recorded but balance untouched.
    let batch_u = monoize::replica::metering::MeteringBatch {
        replica: None,
        request_logs: vec![],
        last_used: vec![],
        balance_deltas: vec![delta("request_charge", &unlimited.id, None, 77)],
    };
    let ack_u = apply_metering_batch(&state.db_pool, &batch_u)
        .await
        .expect("apply u");
    assert_eq!(ack_u.applied_balance_deltas, 1);
    let bal_u = state
        .user_store
        .get_user_balance(&unlimited.id)
        .await
        .expect("bal u")
        .unwrap();
    assert_eq!(bal_u.balance_nano_usd, 0);
}

/// T4: shipment releases spool data only after an HTTP 200 and retains it otherwise.
#[tokio::test]
async fn shipper_acks_delete_and_failures_retain() {
    let temp = TempDir::new().unwrap();
    let spool_dir = temp.path().join("metering");

    // Failing primary first.
    let failing = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_flag = failing.clone();
    let fail_app = Router::new().route(
        monoize::replica::metering::METERING_INGEST_PATH,
        post(move |body: axum::body::Bytes| async move {
            fail_flag.fetch_add(1, Ordering::SeqCst);
            let batch: monoize::replica::metering::MeteringBatch =
                serde_json::from_slice(&body).unwrap();
            assert_eq!(batch.balance_deltas.len(), 1);
            Err::<axum::response::Response, _>((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "boom",
            ))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, fail_app).await.unwrap() });

    let metering =
        ReplicaMetering::new(spool_dir.clone(), 1024 * 1024, &addr, "token", 10).expect("metering");
    metering
        .enqueue_balance_delta("request_charge", "u-1", None, 1234, &serde_json::json!({}))
        .await
        .expect("enqueue");
    assert_eq!(
        metering.pending().outstanding("u-1"),
        1234,
        "M3 pending counter increments on enqueue"
    );

    let log_batcher = RequestLogBatcher::new_with_limits(
        8,
        temp.path().join("rl-spool"),
        64 * 1024 * 1024,
        8 * 1024 * 1024,
        broadcast::channel(4).0,
        Arc::new(dashmap::DashMap::new()),
    );
    let last_used = LastUsedBatcher::with_capacity(16);

    assert_eq!(
        metering.ship_once(&log_batcher, &last_used).await,
        ShipTick::Failure
    );
    assert_eq!(failing.load(Ordering::SeqCst), 1, "one POST attempt made");
    assert_eq!(
        metering.pending().outstanding("u-1"),
        1234,
        "M5 failure retains the pending counter"
    );
    assert_eq!(
        metering.delta_spool().pending_files(),
        1,
        "M5 failure retains the durable delta file"
    );

    // Succeeding primary next: same data ships and is released only after 200.
    let ok_app = Router::new().route(
        monoize::replica::metering::METERING_INGEST_PATH,
        post(|body: axum::body::Bytes| async move {
            let batch: monoize::replica::metering::MeteringBatch =
                serde_json::from_slice(&body).unwrap();
            assert_eq!(batch.balance_deltas[0].amount_nano_usd, "1234");
            axum::Json(monoize::replica::metering::MeteringAck {
                applied_request_logs: batch.request_logs.len() as u64,
                applied_last_used: batch.last_used.len() as u64,
                applied_balance_deltas: batch.balance_deltas.len() as u64,
            })
        }),
    );
    let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = format!("http://{}", listener2.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener2, ok_app).await.unwrap() });

    let metering_ok =
        ReplicaMetering::new(spool_dir, 1024 * 1024, &addr2, "token", 10).expect("metering 2");
    assert_eq!(
        metering_ok.pending().outstanding("u-1"),
        1234,
        "restart restores pending deductions from durable spool"
    );
    assert_eq!(
        metering_ok.ship_once(&log_batcher, &last_used).await,
        ShipTick::Success
    );
    assert_eq!(
        metering_ok.delta_spool().pending_files(),
        0,
        "successful ack deletes the durable file"
    );
    assert_eq!(
        metering_ok.pending().outstanding("u-1"),
        0,
        "successful ack clears the pending deduction"
    );
}

/// T7: a promoted node drains leftover deltas into its own database before serving.
#[tokio::test]
async fn promotion_drain_applies_leftover_deltas_locally() {
    let temp = TempDir::new().unwrap();
    let dsn = format!("sqlite://{}", temp.path().join("m.db").display());
    let state = monoize::app::load_state_with_runtime(test_runtime(dsn))
        .await
        .expect("state");
    let user = state
        .user_store
        .create_user("drain_user", "pw", monoize::users::UserRole::User, None)
        .await
        .expect("user");

    let spool_dir = temp.path().join("leftover-metering");
    std::fs::create_dir_all(&spool_dir).unwrap();
    let spool = DeltaSpool::new(spool_dir.clone(), 1024 * 1024).unwrap();
    spool
        .enqueue(&delta("request_charge", &user.id, None, 250))
        .await
        .expect("enqueue leftover");
    assert_eq!(spool.pending_files(), 1);

    drain_delta_spool_to_local_db(&state.db_pool, &spool)
        .await
        .expect("drain");
    assert_eq!(spool.pending_files(), 0, "PRP9 drain empties the spool");
    let balance = state
        .user_store
        .get_user_balance(&user.id)
        .await
        .expect("bal")
        .unwrap();
    assert_eq!(balance.balance_nano_usd, -250);
}

fn dummy_request_log() -> monoize::users::InsertRequestLog {
    monoize::users::InsertRequestLog {
        request_id: Some(uuid::Uuid::new_v4().to_string()),
        user_id: "u".to_string(),
        api_key_id: None,
        model: "m".to_string(),
        provider_id: None,
        upstream_model: None,
        channel_id: None,
        names: monoize::users::RequestLogNameSnapshots::default(),
        is_stream: false,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: None,
        charge_nano_usd: None,
        status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: None,
        ttfb_ms: None,
        request_ip: None,
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: None,
        effective_provider_type: None,
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn shipper_idle_tick_is_not_a_failure() {
    let temp = TempDir::new().unwrap();
    let metering = ReplicaMetering::new(
        temp.path().join("metering"),
        1024 * 1024,
        "http://127.0.0.1:1",
        "token",
        10,
    )
    .expect("metering");
    let log_batcher = RequestLogBatcher::new_with_limits(
        8,
        temp.path().join("rl-spool"),
        64 * 1024 * 1024,
        8 * 1024 * 1024,
        broadcast::channel(4).0,
        Arc::new(dashmap::DashMap::new()),
    );
    let last_used = LastUsedBatcher::with_capacity(16);
    assert_eq!(
        metering.ship_once(&log_batcher, &last_used).await,
        ShipTick::Idle
    );
}

#[tokio::test]
async fn shipper_does_not_post_a_second_batch_after_combined_failure() {
    let temp = TempDir::new().unwrap();
    let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let posts_flag = posts.clone();
    let fail_app = Router::new().route(
        monoize::replica::metering::METERING_INGEST_PATH,
        post(move |_body: axum::body::Bytes| async move {
            posts_flag.fetch_add(1, Ordering::SeqCst);
            Err::<axum::response::Response, _>((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "boom",
            ))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, fail_app).await.unwrap() });

    let metering = ReplicaMetering::new(
        temp.path().join("metering"),
        1024 * 1024,
        &addr,
        "token",
        10,
    )
    .expect("metering");
    metering
        .enqueue_balance_delta("request_charge", "u-1", None, 9, &serde_json::json!({}))
        .await
        .expect("enqueue");
    let log_batcher = RequestLogBatcher::new_with_limits(
        8,
        temp.path().join("rl-spool"),
        64 * 1024 * 1024,
        8 * 1024 * 1024,
        broadcast::channel(4).0,
        Arc::new(dashmap::DashMap::new()),
    );
    log_batcher
        .push(dummy_request_log())
        .await
        .expect("push log");
    let last_used = LastUsedBatcher::with_capacity(16);
    assert_eq!(
        metering.ship_once(&log_batcher, &last_used).await,
        ShipTick::Failure
    );
    assert_eq!(
        posts.load(Ordering::SeqCst),
        1,
        "M4 allows at most one POST"
    );
}

#[tokio::test]
async fn shipper_caps_last_used_so_batch_stays_under_hard_cap() {
    let temp = TempDir::new().unwrap();
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_flag = seen.clone();
    let ok_app = Router::new().route(
        monoize::replica::metering::METERING_INGEST_PATH,
        post(move |body: axum::body::Bytes| async move {
            let batch: monoize::replica::metering::MeteringBatch =
                serde_json::from_slice(&body).unwrap();
            seen_flag.store(batch.last_used.len(), Ordering::SeqCst);
            assert!(
                batch.request_logs.len() + batch.last_used.len() + batch.balance_deltas.len()
                    <= monoize::replica::metering::METERING_BATCH_HARD_CAP
            );
            axum::Json(monoize::replica::metering::MeteringAck {
                applied_request_logs: batch.request_logs.len() as u64,
                applied_last_used: batch.last_used.len() as u64,
                applied_balance_deltas: batch.balance_deltas.len() as u64,
            })
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, ok_app).await.unwrap() });

    let metering = ReplicaMetering::new(
        temp.path().join("metering"),
        1024 * 1024,
        &addr,
        "token",
        10,
    )
    .expect("metering");
    let log_batcher = RequestLogBatcher::new_with_limits(
        8,
        temp.path().join("rl-spool"),
        64 * 1024 * 1024,
        8 * 1024 * 1024,
        broadcast::channel(4).0,
        Arc::new(dashmap::DashMap::new()),
    );
    let last_used = LastUsedBatcher::with_capacity(3000);
    let now = chrono::Utc::now();
    for i in 0..2001 {
        last_used.record(format!("k{i}"), now);
    }
    assert_eq!(
        metering.ship_once(&log_batcher, &last_used).await,
        ShipTick::Success
    );
    assert_eq!(
        seen.load(Ordering::SeqCst),
        monoize::replica::metering::METERING_BATCH_HARD_CAP
    );
}

#[tokio::test]
async fn t3_ingest_rejects_oversized_batch_without_apply() {
    let (_temp, state) = boot().await;
    let mut node = (*state.node).clone();
    node.replica_token = Some("tok".to_string());
    let mut state = state.with_node_role(NodeRole::Primary);
    state.node = std::sync::Arc::new(node);
    state.metering_token_digest = Some(monoize::replica::metering::sha256_hex_lower("tok"));
    let app = monoize::app::build_app(state);
    let batch = monoize::replica::metering::MeteringBatch {
        replica: None,
        request_logs: vec![],
        last_used: (0..2001)
            .map(|i| monoize::replica::metering::LastUsedPair {
                api_key_id: format!("k{i}"),
                last_used_at: chrono::Utc::now().to_rfc3339(),
            })
            .collect(),
        balance_deltas: vec![],
    };
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(monoize::replica::metering::METERING_INGEST_PATH)
                .header("authorization", "Bearer tok")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&batch).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "metering_batch_too_large");
}

#[tokio::test]
async fn t5_epoch_bump_poll_swap_and_failed_poll_keeps_snapshot() {
    let (_temp, state) = boot().await;
    let snapshot = std::sync::Arc::new(tokio::sync::RwLock::new(
        state.monoize_runtime.read().await.clone(),
    ));
    let mut last = 0u64;
    monoize::replica::poll::apply_config_epoch_tick(
        &state.db_pool,
        &state.settings_store,
        &snapshot,
        &state.custom_transform_store,
        &mut last,
    )
    .await;
    assert_eq!(last, 0);

    let mut settings = state.settings_store.get_all().await.expect("settings");
    settings.monoize_request_timeout_ms = 12_345;
    state
        .settings_store
        .update_all(&settings)
        .await
        .expect("update");
    assert_eq!(
        monoize::settings::read_config_epoch(&state.db_pool)
            .await
            .expect("epoch"),
        1
    );

    monoize::replica::poll::apply_config_epoch_tick(
        &state.db_pool,
        &state.settings_store,
        &snapshot,
        &state.custom_transform_store,
        &mut last,
    )
    .await;
    assert_eq!(last, 1);
    assert_eq!(snapshot.read().await.request_timeout_ms, 12_345);

    let write = state.db_pool.write().await;
    write
        .execute(state.db_pool.stmt(
            "UPDATE state_records SET value = $1 WHERE tenant_id = $2 AND kind = $3 AND id = $4",
            vec![
                "not-a-u64".into(),
                "monoize".into(),
                "config_epoch".into(),
                "global".into(),
            ],
        ))
        .await
        .expect("corrupt epoch");
    drop(write);

    monoize::replica::poll::apply_config_epoch_tick(
        &state.db_pool,
        &state.settings_store,
        &snapshot,
        &state.custom_transform_store,
        &mut last,
    )
    .await;
    assert_eq!(last, 1, "failed poll keeps last applied epoch");
    assert_eq!(
        snapshot.read().await.request_timeout_ms,
        12_345,
        "failed poll keeps prior snapshot"
    );
}

#[tokio::test]
async fn t6_replica_surface_disables_dashboard_and_keeps_api() {
    let (_temp, state) = boot().await;
    let app = monoize::app::build_app(state.with_node_role(NodeRole::Replica));

    async fn json_code(app: axum::Router, uri: &str) -> (axum::http::StatusCode, String) {
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
        (
            status,
            json["error"]["code"].as_str().unwrap_or("").to_string(),
        )
    }

    let (status, code) = json_code(app.clone(), "/").await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(code, "replica_dashboard_disabled");

    let (status, code) = json_code(app.clone(), "/api/dashboard/auth/me").await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(code, "replica_dashboard_disabled");

    let models = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        models
            .into_body()
            .collect()
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes.to_bytes()).ok())
            .and_then(|json| json["error"]["code"].as_str().map(str::to_string))
            .unwrap_or_default(),
        "replica_dashboard_disabled"
    );

    let metrics = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), axum::http::StatusCode::UNAUTHORIZED);
}

/// T9: the persisted identity survives simulated restarts and DeltaSpool cleanup.
#[test]
fn t9_replica_identity_created_once_and_reused_across_restarts() {
    let temp = TempDir::new().unwrap();
    let spool_dir = temp.path().join("metering");

    let first = resolve_replica_identity(None, &spool_dir).expect("first resolution");
    let parsed = uuid::Uuid::parse_str(&first).expect("identity is a UUID");
    assert_eq!(parsed.get_version_num(), 4, "identity is version 4");

    let identity_path = spool_dir.join(REPLICA_IDENTITY_FILE_NAME);
    let content = std::fs::read_to_string(&identity_path).expect("identity file exists");
    assert_eq!(content, format!("{first}\n"), "M9a file format");

    // Simulated restart: resolution over the same directory returns the same identity.
    let second = resolve_replica_identity(None, &spool_dir).expect("second resolution");
    assert_eq!(first, second);

    // M9b: DeltaSpool startup cleanup removes non-json leftovers but keeps the identity.
    std::fs::write(spool_dir.join(".tmp-leftover"), b"junk").unwrap();
    DeltaSpool::new(spool_dir.clone(), 1024 * 1024).expect("spool");
    assert!(identity_path.exists(), "identity survives spool cleanup");
    assert!(
        !spool_dir.join(".tmp-leftover").exists(),
        "temp leftovers are still cleaned"
    );
    let third = resolve_replica_identity(None, &spool_dir).expect("third resolution");
    assert_eq!(first, third);
}

/// T9: `MONOIZE_REPLICA_ID` overrides the file and is validated as a UUID v4.
#[test]
fn t9_replica_identity_env_override_and_validation() {
    let temp = TempDir::new().unwrap();
    let spool_dir = temp.path().join("metering");

    let configured = uuid::Uuid::new_v4();
    let resolved =
        resolve_replica_identity(Some(&configured.to_string().to_uppercase()), &spool_dir)
            .expect("override resolves");
    assert_eq!(
        resolved,
        configured.hyphenated().to_string(),
        "override canonicalizes to lowercase hyphenated form"
    );
    assert!(
        !spool_dir.join(REPLICA_IDENTITY_FILE_NAME).exists(),
        "override neither reads nor writes the identity file"
    );

    let err = resolve_replica_identity(Some("not-a-uuid"), &spool_dir).unwrap_err();
    assert!(err.starts_with("replica_id_invalid"), "{err}");
    // Nil UUID parses but is not version 4.
    let err = resolve_replica_identity(Some("00000000-0000-0000-0000-000000000000"), &spool_dir)
        .unwrap_err();
    assert!(err.starts_with("replica_id_invalid"), "{err}");
}

/// T9: a corrupt identity file is replaced by a freshly generated identity.
#[test]
fn t9_replica_identity_regenerates_corrupt_file() {
    let temp = TempDir::new().unwrap();
    let spool_dir = temp.path().join("metering");
    std::fs::create_dir_all(&spool_dir).unwrap();
    let identity_path = spool_dir.join(REPLICA_IDENTITY_FILE_NAME);
    std::fs::write(&identity_path, b"garbage\n").unwrap();

    let regenerated = resolve_replica_identity(None, &spool_dir).expect("regenerates");
    let parsed = uuid::Uuid::parse_str(&regenerated).expect("valid UUID");
    assert_eq!(parsed.get_version_num(), 4);
    assert_eq!(
        std::fs::read_to_string(&identity_path).unwrap(),
        format!("{regenerated}\n"),
        "corrupt file is atomically replaced"
    );
    assert_eq!(
        resolve_replica_identity(None, &spool_dir).unwrap(),
        regenerated
    );
}

fn heartbeat_record(id: &str, last_seen_unix_ms: i64) -> ReplicaHeartbeatRecord {
    ReplicaHeartbeatRecord {
        heartbeat: ReplicaHeartbeat {
            id: id.to_string(),
            hostname: "host".to_string(),
            listen: "0.0.0.0:9000".to_string(),
            version: "1.0.0".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            uptime_seconds: 0,
            spool_pending_count: 0,
            spool_pending_bytes: 0,
        },
        last_seen_unix_ms,
    }
}

/// T10: overview reads evict entries older than 360 ship intervals and keep the rest.
#[test]
fn t10_heartbeat_eviction_removes_only_expired_entries() {
    let ship_interval = std::time::Duration::from_secs(10);
    let evict_after_ms = ship_interval.as_millis() as i64 * HEARTBEAT_EVICT_INTERVALS as i64;
    let now_ms = chrono::Utc::now().timestamp_millis();

    let map = dashmap::DashMap::new();
    map.insert(
        "expired".to_string(),
        heartbeat_record("expired", now_ms - evict_after_ms - 1),
    );
    map.insert(
        "stale-but-kept".to_string(),
        heartbeat_record("stale-but-kept", now_ms - evict_after_ms),
    );
    map.insert("live".to_string(), heartbeat_record("live", now_ms));

    evict_expired_heartbeats(&map, now_ms, ship_interval);

    assert!(!map.contains_key("expired"), "expired entry is removed");
    assert!(
        map.contains_key("stale-but-kept"),
        "boundary entry is retained"
    );
    assert!(map.contains_key("live"), "live entry is retained");
}

#[tokio::test]
async fn t8_postgres_ingest_parity_when_configured() {
    let Some(dsn) = std::env::var("MONOIZE_TEST_POSTGRES_DSN")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };
    let temp = TempDir::new().unwrap();
    let mut runtime = test_runtime(dsn);
    runtime.node.metering_spool_dir = temp.path().join("metering");
    let state = monoize::app::load_state_with_runtime(runtime)
        .await
        .expect("postgres state loads");

    let column = state
        .db_pool
        .read()
        .query_one(state.db_pool.stmt(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = 'billing_ledger' AND column_name = 'idempotency_key'",
            vec![],
        ))
        .await
        .expect("schema query");
    assert!(column.is_some(), "SC1 idempotency_key exists on postgres");

    let user = state
        .user_store
        .create_user(
            &format!("pg_{}", uuid::Uuid::new_v4().simple()),
            "pw",
            monoize::users::UserRole::User,
            None,
        )
        .await
        .expect("user");
    state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("5000000000"),
            None,
            None,
            None,
        )
        .await
        .expect("seed");
    let batch = monoize::replica::metering::MeteringBatch {
        replica: None,
        request_logs: vec![],
        last_used: vec![],
        balance_deltas: vec![delta("request_charge", &user.id, None, 1_000_000_000)],
    };
    let ack1 = apply_metering_batch(&state.db_pool, &batch)
        .await
        .expect("apply");
    assert_eq!(ack1.applied_balance_deltas, 1);
    let ack2 = apply_metering_batch(&state.db_pool, &batch)
        .await
        .expect("replay");
    assert_eq!(ack2.applied_balance_deltas, 0);
    let balance = state
        .user_store
        .get_user_balance_uncached(&user.id)
        .await
        .expect("balance")
        .expect("row");
    assert_eq!(balance.balance_nano_usd, 4_000_000_000);
}

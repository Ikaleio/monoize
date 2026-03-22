use super::*;

#[tokio::test]
async fn chat_streaming_records_ttfb_usage_and_charge_in_request_logs() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream-log"}],
                "stream": true,
                "emit_usage": true
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None, None, None)
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.tokens.input == Some(12)
                && log.tokens.output == Some(8)
                && log.billing.charge_nano_usd.as_deref() == Some("20000")
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("request log should be inserted");
    assert!(log.is_stream);
    assert!(log.timing.ttfb_ms.is_some());
    assert_eq!(log.tokens.input, Some(12));
    assert_eq!(log.tokens.output, Some(8));
    assert_eq!(log.billing.charge_nano_usd.as_deref(), Some("20000"));
}

#[tokio::test]
async fn chat_streaming_requests_upstream_include_usage_by_default() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"stream-log-include-usage"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some("gpt-5-mini-chat"),
                Some("success"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.tokens.input == Some(12)
                && log.tokens.output == Some(8)
                && log.billing.charge_nano_usd.as_deref() == Some("20000")
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("stream log should include usage without explicit emit_usage");
    assert!(log.timing.ttfb_ms.is_some());
}

#[tokio::test]
async fn request_logs_pending_transitions_to_success_and_charges_once() {
    let ctx = setup().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("set finite balance");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user before")
        .expect("user exists");

    let router = ctx.router.clone();
    let auth_header = ctx.auth_header.clone();
    let request_task = tokio::spawn(async move {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, auth_header)
            .body(Body::from(
                json!({
                    "model":"gpt-5-mini-chat",
                    "messages":[{"role":"user","content":"pending-transition"}],
                    "stream": true,
                    "emit_usage": true,
                    "force_upstream_delay_ms": 800
                })
                .to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = resp.into_body().collect().await.unwrap().to_bytes();
    });

    request_task.await.expect("request task");
    ctx.state.user_store.flush_all_batchers().await;

    let mut model_logs = Vec::new();
    for _ in 0..30 {
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None, None, None)
            .await
            .expect("list request logs");
        model_logs = logs
            .into_iter()
            .filter(|log| log.model == "gpt-5-mini-chat")
            .collect();
        if model_logs.len() == 1 && model_logs[0].status == "success" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    assert_eq!(
        model_logs.len(),
        1,
        "same request should keep a single lifecycle row"
    );
    let log = &model_logs[0];
    assert_eq!(log.status, "success");
    assert_eq!(log.billing.charge_nano_usd.as_deref(), Some("20000"));

    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user after")
        .expect("user exists");
    let before_nano: i128 = user_before
        .balance_nano_usd
        .parse()
        .expect("parse before balance");
    let after_nano: i128 = user_after
        .balance_nano_usd
        .parse()
        .expect("parse after balance");
    assert_eq!(before_nano - after_nano, 20000);
}

#[tokio::test]
async fn request_logs_pending_usage_can_be_updated_incrementally() {
    // With the batcher pattern, insert_request_log_pending and
    // update_pending_request_log_usage are no-ops. Verify they succeed
    // without error but produce no persisted row.
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");
    let request_id = "pending-usage-update-test";

    ctx.state
        .user_store
        .insert_request_log_pending(request_id, &user.id, None, "gpt-5-mini-chat", true, None)
        .await
        .expect("insert_request_log_pending should succeed (no-op)");

    ctx.state
        .user_store
        .update_pending_request_log_usage(
            &user.id,
            request_id,
            12,
            8,
            Some(0),
            Some(3),
            Some(2),
            Some(0),
            Some(5),
            Some(1),
            Some(json!({
                "input": { "total_tokens": 12 },
                "output": { "total_tokens": 8 }
            })),
        )
        .await
        .expect("update_pending_request_log_usage should succeed (no-op)");

    // No row should be persisted since both methods are no-ops under batcher pattern
    let (logs, _, _) = ctx
        .state
        .user_store
        .list_request_logs_by_user(
            &user.id,
            100,
            0,
            Some("gpt-5-mini-chat"),
            Some("pending"),
            None,
            Some(request_id),
            None,
            None,
        )
        .await
        .expect("list pending logs");

    assert!(
        logs.is_empty(),
        "no pending row should exist under batcher pattern"
    );
}

#[tokio::test]
async fn request_log_batcher_broadcasts_immediately_before_flush() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut receiver = ctx.state.log_broadcast.subscribe();
    let log = monoize::users::InsertRequestLog {
        request_id: Some("immediate-broadcast-request".to_string()),
        user_id: user.id.clone(),
        api_key_id: None,
        model: "gpt-5-mini-chat".to_string(),
        provider_id: None,
        upstream_model: None,
        channel_id: None,
        is_stream: false,
        input_tokens: Some(12),
        output_tokens: Some(8),
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: None,
        charge_nano_usd: Some(1234),
        status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: Some(50),
        ttfb_ms: None,
        request_ip: Some("127.0.0.1".to_string()),
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: None,
        created_at: chrono::Utc::now(),
    };

    ctx.state
        .user_store
        .finalize_request_log(log.clone())
        .await
        .expect("enqueue request log");

    let batch = tokio::time::timeout(Duration::from_millis(200), receiver.recv())
        .await
        .expect("broadcast should not wait for batch flush")
        .expect("broadcast channel should deliver batch");

    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].request_id.as_deref(),
        Some("immediate-broadcast-request")
    );
    assert_eq!(batch[0].status, monoize::users::REQUEST_LOG_STATUS_SUCCESS);

    ctx.state.user_store.flush_all_batchers().await;

    let duplicate = tokio::time::timeout(Duration::from_millis(150), receiver.recv()).await;
    assert!(
        duplicate.is_err(),
        "flushing persisted request logs should not rebroadcast duplicate terminal events"
    );
}

#[tokio::test]
async fn chat_upstream_error_is_logged_and_not_billed() {
    let ctx = setup().await;

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user before")
        .expect("user exists");

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"force error"}],
            "force_upstream_error_status": 422,
            "force_upstream_error_code": "rate_limit"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        ctx.state.user_store.flush_all_batchers().await;
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some("gpt-5-mini-chat"),
                Some("error"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        matched = logs
            .into_iter()
            .find(|log| log.status == "error" && log.error.http_status == Some(422));
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("error request log should be inserted");
    assert_eq!(log.billing.charge_nano_usd, None);
    assert_eq!(log.error.code.as_deref(), Some("upstream_error"));
    assert!(
        log.error
            .message
            .as_deref()
            .unwrap_or("")
            .contains("upstream status 422")
    );

    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user after")
        .expect("user exists");
    assert_eq!(user_before.balance_nano_usd, user_after.balance_nano_usd);
}

#[tokio::test]
async fn request_log_retention_deletes_only_rows_older_than_ninety_days() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let old_created_at = Utc::now() - ChronoDuration::days(91);
    let new_created_at = Utc::now() - ChronoDuration::days(30);

    ctx.state
        .user_store
        .finalize_request_log(monoize::users::InsertRequestLog {
            request_id: Some("retention-old".to_string()),
            user_id: user.id.clone(),
            api_key_id: None,
            model: "retention-model-old".to_string(),
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream: false,
            input_tokens: Some(1),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: Some(1),
            status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(1),
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: None,
            created_at: old_created_at,
        })
        .await
        .expect("insert old request log");

    ctx.state
        .user_store
        .finalize_request_log(monoize::users::InsertRequestLog {
            request_id: Some("retention-new".to_string()),
            user_id: user.id.clone(),
            api_key_id: None,
            model: "retention-model-new".to_string(),
            provider_id: None,
            upstream_model: None,
            channel_id: None,
            is_stream: false,
            input_tokens: Some(1),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            tool_prompt_tokens: None,
            reasoning_tokens: None,
            accepted_prediction_tokens: None,
            rejected_prediction_tokens: None,
            provider_multiplier: None,
            charge_nano_usd: Some(1),
            status: monoize::users::REQUEST_LOG_STATUS_SUCCESS.to_string(),
            usage_breakdown_json: None,
            billing_breakdown_json: None,
            error_code: None,
            error_message: None,
            error_http_status: None,
            duration_ms: Some(1),
            ttfb_ms: None,
            request_ip: None,
            reasoning_effort: None,
            tried_providers_json: None,
            request_kind: None,
            created_at: new_created_at,
        })
        .await
        .expect("insert new request log");

    ctx.state.user_store.flush_all_batchers().await;

    let deleted = ctx
        .state
        .user_store
        .cleanup_expired_request_logs()
        .await
        .expect("cleanup expired request logs");
    assert_eq!(deleted, 1, "only logs older than 90 days should be deleted");

    let (logs, _, _) = ctx
        .state
        .user_store
        .list_request_logs_by_user(&user.id, 100, 0, None, None, None, None, None, None)
        .await
        .expect("list request logs after retention cleanup");

    assert!(
        logs.iter()
            .all(|log| log.request_id.as_deref() != Some("retention-old")),
        "expired log should be removed"
    );
    assert!(
        logs.iter()
            .any(|log| log.request_id.as_deref() == Some("retention-new")),
        "recent log should remain"
    );
}

#[tokio::test]
async fn chat_streaming_length_finish_is_still_billed() {
    let ctx = setup().await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"length-billing"}],
                "stream": true,
                "emit_usage": true,
                "force_finish_reason": "length"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap().to_bytes();
    ctx.state.user_store.flush_all_batchers().await;

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("query user")
        .expect("user exists");

    let mut matched = None;
    for _ in 0..20 {
        let (logs, _, _) = ctx
            .state
            .user_store
            .list_request_logs_by_user(
                &user.id,
                100,
                0,
                Some("gpt-5-mini-chat"),
                Some("success"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("list request logs");
        matched = logs.into_iter().find(|log| {
            log.model == "gpt-5-mini-chat"
                && log.is_stream
                && log.tokens.input == Some(12)
                && log.tokens.output == Some(8)
                && log.billing.charge_nano_usd.as_deref() == Some("20000")
                && log.status == "success"
        });
        if matched.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let log = matched.expect("stream length request should be billed");
    assert_eq!(log.status, "success");
    assert_eq!(log.billing.charge_nano_usd.as_deref(), Some("20000"));
}

#[tokio::test]
async fn billing_injected_usage_field_ignored() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"billing usage injection"}],
            "stream": true,
            "emit_usage": true,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn billing_injected_pricing_field_ignored() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"billing pricing injection"}],
            "stream": true,
            "emit_usage": true,
            "pricing": {"input_cost": 0}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn billing_model_field_does_not_affect_upstream_charge() {
    let ctx = setup().await;

    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers");
    let base_url = providers
        .iter()
        .find_map(|p| p.channels.first().map(|c| c.base_url.clone()))
        .expect("base_url");

    let mut models = HashMap::new();
    models.insert(
        "alias-route-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: Some("gpt-5-mini".to_string()),
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "alias-route-provider".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("alias-route-ch".to_string()),
                name: "alias-route-ch".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-50),
        })
        .await
        .expect("create alias provider");

    ctx.state
        .model_registry_store
        .upsert_model_metadata(
            "alias-route-model",
            monoize::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some("test".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("999999".to_string()),
                output_cost_per_token_nano: Some("999999".to_string()),
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("seed alias model pricing");

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model":"alias-route-model",
            "input":"route-charge",
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20000);
}

#[tokio::test]
async fn redirected_model_pricing_falls_back_to_logical_model_when_upstream_unpriced() {
    let ctx = setup().await;

    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers");
    let base_url = providers
        .iter()
        .find_map(|p| p.channels.first().map(|c| c.base_url.clone()))
        .expect("base_url");

    let mut models = HashMap::new();
    models.insert(
        "alias-fallback-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: Some("gpt-5-unpriced-upstream".to_string()),
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "alias-fallback-provider".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("alias-fallback-ch".to_string()),
                name: "alias-fallback-ch".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-50),
        })
        .await
        .expect("create alias fallback provider");

    ctx.state
        .model_registry_store
        .upsert_model_metadata(
            "alias-fallback-model",
            monoize::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some("test".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("2000".to_string()),
                output_cost_per_token_nano: Some("3000".to_string()),
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("seed alias fallback model pricing");

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model":"alias-fallback-model",
            "input":"route-charge-fallback",
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 48_000);
}

#[tokio::test]
async fn suffixed_model_pricing_uses_base_model_metadata_without_separate_alias_pricing() {
    let ctx = setup().await;

    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers");
    let base_url = providers
        .iter()
        .find_map(|p| p.channels.first().map(|c| c.base_url.clone()))
        .expect("base_url");

    let mut models = HashMap::new();
    models.insert(
        "gpt-5-mini-thinking".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    ctx.state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "suffix-pricing-provider".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("suffix-pricing-ch".to_string()),
                name: "suffix-pricing-ch".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-50),
        })
        .await
        .expect("create suffix provider");

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let (status, _body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model":"gpt-5-mini-thinking",
            "input":"suffix-charge",
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();

    assert_eq!(before - after, 20_000);
}

#[tokio::test]
async fn balance_zero_returns_payment_required() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("0"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({"model":"gpt-5-mini","input":"hi"}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("insufficient_balance"));
}

#[tokio::test]
async fn balance_exact_covers_request() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("20000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"hi"}],
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();
    assert_eq!(after, 0);
}

#[tokio::test]
async fn balance_insufficient_after_charge() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("10000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"gpt-5-mini-chat",
            "messages":[{"role":"user","content":"hi"}],
            "stream": true,
            "emit_usage": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("[DONE]"));

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();
    assert_eq!(after, 10000);
}

#[tokio::test]
async fn extra_fields_do_not_corrupt_response() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    ctx.state
        .user_store
        .update_user(
            &user.id,
            None,
            None,
            None,
            None,
            Some("1000000000"),
            Some(false),
            None,
        )
        .await
        .expect("update user");

    let user_before = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let before: i64 = user_before.balance_nano_usd.parse().unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, ctx.auth_header.clone())
        .body(Body::from(
            json!({
                "model":"gpt-5-mini-chat",
                "messages":[{"role":"user","content":"edge-extra-fields"}],
                "stream": true,
                "emit_usage": true,
                "hack_model":"free-model",
                "override_billing": true,
                "admin": true,
                "nested": {"inject": [1,2,3], "bypass": "no"}
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("edge-extra-fields"));

    ctx.state.user_store.flush_all_batchers().await;
    let user_after = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let after: i64 = user_after.balance_nano_usd.parse().unwrap();
    assert_eq!(before - after, 20000);
}

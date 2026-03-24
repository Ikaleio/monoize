use super::*;

#[tokio::test]
async fn models_list_returns_union_sorted_and_unique() {
    let ctx = setup().await;

    create_test_provider(
        &ctx.state,
        "up-dup",
        monoize::monoize_routing::MonoizeProviderType::Responses,
        "gpt-5-mini",
        "http://127.0.0.1:1",
        "upstream-key",
    )
    .await;
    create_test_provider(
        &ctx.state,
        "up-new",
        monoize::monoize_routing::MonoizeProviderType::Responses,
        "zeta-model",
        "http://127.0.0.1:1",
        "upstream-key",
    )
    .await;

    let (status, body) = json_get(&ctx, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);

    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "list");
    let data = v["data"].as_array().expect("data should be an array");

    let ids: Vec<String> = data
        .iter()
        .map(|item| {
            assert_eq!(item["object"], "model");
            assert_eq!(item["created"], 0);
            assert_eq!(item["owned_by"], "monoize");
            item["id"]
                .as_str()
                .expect("id should be string")
                .to_string()
        })
        .collect();

    assert_eq!(
        ids,
        vec![
            "gemini-2.5-flash".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5-mini-chat".to_string(),
            "gpt-5-mini-msg".to_string(),
            "grok-4".to_string(),
            "zeta-model".to_string(),
        ]
    );
}

#[tokio::test]
async fn models_list_api_alias_works() {
    let ctx = setup().await;
    let (status, body) = json_get(&ctx, "/api/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "list");
}

#[tokio::test]
async fn api_alias_works() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/api/v1/responses",
        json!({"model":"gpt-5-mini","input":"hello"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("response"));
}

#[tokio::test]
async fn channel_passive_override_threshold_takes_precedence_over_global_defaults() {
    let ctx = setup().await;
    seed_test_model_pricing(&ctx.state, &["override-threshold-model"]).await;

    let providers = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers");
    let base_url = providers
        .iter()
        .find_map(|p| p.channels.first().map(|c| c.base_url.clone()))
        .expect("at least one existing channel base url");

    let mut models = HashMap::new();
    models.insert(
        "override-threshold-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: None,
            multiplier: 1.0,
        },
    );
    let created = ctx
        .state
        .monoize_store
        .create_provider(monoize::monoize_routing::CreateMonoizeProviderInput {
            name: "override-threshold-provider".to_string(),
            provider_type: monoize::monoize_routing::MonoizeProviderType::ChatCompletion,
            models,
            api_type_overrides: Vec::new(),
            channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
                id: Some("override-threshold-ch".to_string()),
                name: "override-threshold-ch".to_string(),
                base_url,
                api_key: Some("upstream-key".to_string()),
                weight: 1,
                enabled: true,
                groups: Vec::new(),
                passive_failure_count_threshold_override: Some(1),
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            enabled: true,
            priority: Some(-10),
        })
        .await
        .expect("create provider with channel override");
    let channel_id = created.channels[0].id.clone();

    let (status, _body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model":"override-threshold-model",
            "messages":[{"role":"user","content":"trigger retryable failure"}],
            "force_upstream_error_status": 500,
            "force_upstream_error_code": "forced_500"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let health = ctx.state.channel_health.lock().await;
    let state = health
        .get(&channel_id)
        .cloned()
        .expect("channel health state exists");
    assert!(
        !state.healthy,
        "channel should become unhealthy after one transient failure when override threshold=1"
    );
    assert_eq!(
        state
            .passive_samples
            .iter()
            .filter(|sample| sample.failed)
            .count(),
        1,
        "one failed sample should be recorded in the passive breaker window"
    );
}

#[tokio::test]
async fn provider_request_transform_matches_normalized_model_before_redirect() {
    let ctx = setup().await;
    seed_test_model_pricing(&ctx.state, &["gpt-5-target"]).await;
    let (upstream_addr, _) = start_upstream().await;
    let base_url = format!("http://{upstream_addr}");

    let mut models = HashMap::new();
    models.insert(
        "normalized-transform-model".to_string(),
        monoize::monoize_routing::MonoizeModelEntry {
            redirect: Some("gpt-5-target".to_string()),
            multiplier: 1.0,
        },
    );

    let create_input = monoize::monoize_routing::CreateMonoizeProviderInput {
        name: "mono-transform-original-model-match".to_string(),
        provider_type: monoize::monoize_routing::MonoizeProviderType::Responses,
        models,
        api_type_overrides: Vec::new(),
        channels: vec![monoize::monoize_routing::CreateMonoizeChannelInput {
            id: Some("mono-transform-original-model-match-ch1".to_string()),
            name: "mono-transform-original-model-match-ch1".to_string(),
            base_url,
            api_key: Some("upstream-key".to_string()),
            weight: 1,
            enabled: true,
            groups: Vec::new(),
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
        }],
        max_retries: -1,
        channel_max_retries: 0,
        channel_retry_interval_ms: 0,
        circuit_breaker_enabled: true,
        per_model_circuit_break: false,
        transforms: vec![monoize::transforms::TransformRuleConfig {
            transform: "set_field".to_string(),
            enabled: true,
            models: Some(vec!["normalized-transform-model".to_string()]),
            phase: monoize::transforms::Phase::Request,
            config: json!({
                "path": "extra_echo",
                "value": "matched-original-model"
            }),
        }],
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        request_timeout_ms_override: None,
        enabled: true,
        priority: Some(-1),
    };

    ctx.state
        .monoize_store
        .create_provider(create_input)
        .await
        .unwrap();

    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "normalized-transform-model-high",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hello" }] }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let text = v["output"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["type"].as_str() == Some("message"))
        })
        .and_then(|item| item["content"].as_array())
        .and_then(|content| content.first())
        .and_then(|part| part["text"].as_str())
        .unwrap_or("");
    assert!(
        text.contains("extra_echo=matched-original-model"),
        "expected request transform to match normalized logical model before redirect: text={text}; body={body}"
    );
}

#[tokio::test]
async fn models_list_respects_api_key_model_limits() {
    let ctx = setup().await;

    let (status, body) = json_get(&ctx, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let all_ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(all_ids.len() > 2, "should have multiple models");

    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .expect("get user")
        .expect("user exists");
    let (_, restricted_token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "restricted-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: true,
                model_limits: vec!["gpt-5-mini".to_string(), "grok-4".to_string()],
                ip_whitelist: Vec::new(),
                group: "default".to_string(),
                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
            false,
        )
        .await
        .expect("create restricted api key");

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .header(AUTHORIZATION, format!("Bearer {restricted_token}"))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    let restricted_ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(restricted_ids, vec!["gpt-5-mini", "grok-4"]);
}

#[tokio::test]
async fn models_list_model_limits_disabled_shows_all() {
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
                name: "disabled-limits-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: vec!["gpt-5-mini".to_string()],
                ip_whitelist: Vec::new(),
                group: "default".to_string(),
                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
            },
            false,
        )
        .await
        .expect("create api key with disabled limits");

    let req = Request::builder()
        .method("GET")
        .uri("/v1/models")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap();
    let ids: Vec<String> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();

    assert!(
        ids.len() > 1,
        "should return all models when limits disabled"
    );
}

#[tokio::test]
async fn forwarding_rejects_models_outside_api_key_model_limits() {
    let ctx = setup().await;
    let user = ctx
        .state
        .user_store
        .get_user_by_username("tenant-1")
        .await
        .unwrap()
        .unwrap();

    let (_, token) = ctx
        .state
        .user_store
        .create_api_key_extended(
            &user.id,
            monoize::users::CreateApiKeyInput {
                name: "restricted-forward-key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: true,
                model_limits: vec!["gpt-5-mini".to_string()],
                ip_whitelist: vec![],
                group: "default".to_string(),
                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: vec![],
            },
            false,
        )
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({
                "model": "grok-4",
                "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = ctx.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("model_not_allowed"));
}

#[tokio::test]
async fn unknown_model_returns_error() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({"model":"nonexistent-model-xyz","input":"hi"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"].as_str(), Some("upstream_error"));
}

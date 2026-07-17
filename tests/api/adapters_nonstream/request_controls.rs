async fn set_messages_metadata_collision_transform(ctx: &TestContext) {
    let provider = ctx
        .state
        .monoize_store
        .list_providers()
        .await
        .expect("list providers")
        .into_iter()
        .find(|provider| provider.name == "up-msg")
        .expect("messages provider");

    ctx.state
        .monoize_store
        .update_provider(
            &provider.id,
            monoize::monoize_routing::UpdateMonoizeProviderInput {
                name: None,
                models: None,
                channels: None,
                max_retries: None,
                channel_max_retries: None,
                channel_retry_interval_ms: None,
                circuit_breaker_enabled: None,
                per_model_circuit_break: None,
                transforms: Some(vec![monoize::transforms::TransformRuleConfig {
                    transform: "set_field".to_string(),
                    enabled: true,
                    models: None,
                    phase: monoize::transforms::Phase::Request,
                    config: json!({
                        "path": "metadata.user_id",
                        "value": "transform-collision"
                    }),
                }]),
                active_probe_enabled_override: None,
                api_type_overrides: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                request_timeout_ms_override: None,
                extra_fields_whitelist: None,
                strip_cross_protocol_nested_extra: None,
                groups: None,
                enabled: None,
                priority: None,
            },
        )
        .await
        .expect("install messages request transform");
}

#[tokio::test]
async fn chat_request_controls_map_to_messages_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-msg",
            "messages": [{ "role": "user", "content": "map controls" }],
            "stop": "END",
            "verbosity": "high",
            "user": "chat-user"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["stop_sequences"], json!(["END"]));
    assert_eq!(upstream["metadata"]["user_id"], json!("chat-user"));
    assert!(upstream.get("stop").is_none(), "{upstream}");
    assert!(upstream.get("verbosity").is_none(), "{upstream}");
}

#[tokio::test]
async fn chat_stop_shape_round_trips_same_family() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "scalar stop" }],
            "stop": "SCALAR"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(last_captured_body(&ctx, "chat")["stop"], json!("SCALAR"));

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-chat",
            "messages": [{ "role": "user", "content": "array stop" }],
            "stop": ["ONE", "TWO"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        last_captured_body(&ctx, "chat")["stop"],
        json!(["ONE", "TWO"])
    );
}

#[tokio::test]
async fn messages_request_controls_map_to_chat_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-chat",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "map controls" }] }],
            "stop_sequences": ["FIRST", "SECOND"],
            "metadata": {
                "user_id": "messages-user",
                "trace_id": "trace-cross-family"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "chat");
    assert_eq!(upstream["stop"], json!(["FIRST", "SECOND"]));
    assert_eq!(upstream["user"], json!("messages-user"));
    assert!(upstream.get("stop_sequences").is_none(), "{upstream}");
}

#[tokio::test]
async fn chat_and_responses_verbosity_map_in_both_directions() {
    let to_responses = setup().await;
    let (status, body) = json_post(
        &to_responses,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini",
            "messages": [{ "role": "user", "content": "chat to responses" }],
            "verbosity": "low",
            "user": "chat-responses-user",
            "stop": ["OMIT", "IN_RESPONSES"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let responses_upstream = last_captured_body(&to_responses, "responses");
    assert_eq!(responses_upstream["text"]["verbosity"], json!("low"));
    assert_eq!(responses_upstream["user"], json!("chat-responses-user"));
    assert!(responses_upstream.get("stop").is_none(), "{responses_upstream}");
    assert!(
        responses_upstream.get("stop_sequences").is_none(),
        "{responses_upstream}"
    );

    let to_chat = setup().await;
    let (status, body) = json_post(
        &to_chat,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-chat",
            "input": "responses to chat",
            "text": {
                "verbosity": "medium",
                "future_text_control": { "mode": "preserve" }
            },
            "user": "responses-chat-user"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let chat_upstream = last_captured_body(&to_chat, "chat");
    assert_eq!(chat_upstream["verbosity"], json!("medium"));
    assert_eq!(chat_upstream["user"], json!("responses-chat-user"));
}

#[tokio::test]
async fn messages_metadata_siblings_survive_and_typed_user_wins() {
    let ctx = setup().await;
    set_messages_metadata_collision_transform(&ctx).await;

    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "metadata" }] }],
            "metadata": {
                "user_id": "typed-source-user",
                "trace_id": "trace-same-family",
                "future_metadata": { "enabled": true }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["metadata"]["user_id"], json!("typed-source-user"));
    assert_eq!(
        upstream["metadata"]["trace_id"],
        json!("trace-same-family")
    );
    assert_eq!(
        upstream["metadata"]["future_metadata"],
        json!({ "enabled": true })
    );
}

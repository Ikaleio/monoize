/// When a downstream `/v1/messages` request echoes back an assistant `thinking` block with
/// plaintext content and signature (the common Claude round-trip case), monoize must forward
/// both fields verbatim to the upstream Messages provider. Signature integrity is critical
/// for newer Claude models (Sonnet 4.x and Opus 4.x) where `signature` is the encrypted
/// reasoning payload, not a verifier.
#[tokio::test]
async fn messages_request_preserves_thinking_and_signature_through_messages_upstream() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "prior reasoning text",
                            "signature": "encrypted_reasoning_blob"
                        },
                        { "type": "text", "text": "prior answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let thinking_block = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("thinking"))
        .expect("thinking block forwarded");
    assert_eq!(thinking_block["thinking"], "prior reasoning text");
    assert_eq!(thinking_block["signature"], "encrypted_reasoning_blob");
}

#[tokio::test]
async fn messages_request_preserves_explicit_thinking_and_output_config() {
    let ctx = setup().await;
    let thinking = json!({
        "type": "disabled",
        "vendor_control": { "mode": "exact" }
    });
    let output_config = json!({
        "effort": "high",
        "format": {
            "type": "json_schema",
            "schema": {
                "type": "object",
                "properties": { "answer": { "type": "string" } },
                "required": ["answer"]
            }
        },
        "vendor_control": ["preserve", "verbatim"]
    });
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "thinking": thinking.clone(),
            "output_config": output_config.clone(),
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "preserve controls" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(upstream["thinking"], thinking);
    assert_eq!(upstream["output_config"], output_config);
}

#[tokio::test]
async fn messages_thinking_validation_rejects_before_upstream_dispatch() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 4096,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": "valid manual thinking" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let captured_after_valid = ctx.captured_bodies.lock().expect("captured bodies").len();

    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 2048,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": "invalid manual thinking" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let error: Value = serde_json::from_str(&body).expect("Messages error JSON");
    assert_eq!(error["type"], json!("error"));
    assert_eq!(error["error"]["type"], json!("invalid_request_error"));
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("must be less than max_tokens")),
        "unexpected validation error: {error}"
    );
    assert_eq!(
        ctx.captured_bodies.lock().expect("captured bodies").len(),
        captured_after_valid,
        "invalid explicit thinking must not reach upstream"
    );

    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-msg",
            "messages": [{ "role": "user", "content": "invalid generated thinking" }],
            "max_completion_tokens": 64,
            "reasoning_effort": "high"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let error: Value = serde_json::from_str(&body).expect("Chat error JSON");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("must be less than max_tokens")),
        "unexpected generated-control validation error: {error}"
    );
    assert_eq!(
        ctx.captured_bodies.lock().expect("captured bodies").len(),
        captured_after_valid,
        "invalid generated thinking must not reach upstream"
    );
}

#[tokio::test]
async fn messages_structured_output_round_trips_and_maps_to_openai_families() {
    let ctx = setup().await;
    let schema = json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"],
        "additionalProperties": false
    });
    let output_config = json!({
        "effort": "high",
        "format": {
            "type": "json_schema",
            "schema": schema.clone(),
            "messages_extension": { "mode": "exact" }
        },
        "vendor_control": ["preserve", "verbatim"]
    });

    for (model, target) in [
        ("gpt-5-mini-msg", "messages"),
        ("gpt-5-mini-chat", "chat"),
        ("gpt-5-mini", "responses"),
    ] {
        let (status, body) = json_post(
            &ctx,
            "/v1/messages",
            json!({
                "model": model,
                "max_tokens": 64,
                "output_config": output_config.clone(),
                "messages": [{ "role": "user", "content": "structured answer" }]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "target={target}: {body}");

        let upstream = last_captured_body(&ctx, target);
        match target {
            "messages" => assert_eq!(upstream["output_config"], output_config),
            "chat" => {
                assert_eq!(upstream["response_format"]["type"], json!("json_schema"));
                assert_eq!(
                    upstream["response_format"]["json_schema"],
                    json!({ "name": "response", "schema": schema.clone() })
                );
            }
            "responses" => {
                assert_eq!(
                    upstream["text"]["format"],
                    json!({
                        "type": "json_schema",
                        "name": "response",
                        "schema": schema.clone()
                    })
                );
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn openai_json_schemas_map_to_messages_output_config_format() {
    let ctx = setup().await;
    let chat_schema = json!({
        "type": "object",
        "properties": { "chat": { "type": "boolean" } },
        "required": ["chat"]
    });
    let (status, body) = json_post(
        &ctx,
        "/v1/chat/completions",
        json!({
            "model": "gpt-5-mini-msg",
            "messages": [{ "role": "user", "content": "chat schema" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "chat_answer",
                    "description": "OpenAI-only description",
                    "schema": chat_schema.clone(),
                    "strict": true
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(
        upstream["output_config"]["format"],
        json!({ "type": "json_schema", "schema": chat_schema })
    );

    let responses_schema = json!({
        "type": "object",
        "properties": { "responses": { "type": "integer" } },
        "required": ["responses"]
    });
    let (status, body) = json_post(
        &ctx,
        "/v1/responses",
        json!({
            "model": "gpt-5-mini-msg",
            "input": "responses schema",
            "text": {
                "verbosity": "low",
                "format": {
                    "type": "json_schema",
                    "name": "responses_answer",
                    "description": "OpenAI-only description",
                    "schema": responses_schema.clone(),
                    "strict": true
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let upstream = last_captured_body(&ctx, "messages");
    assert_eq!(
        upstream["output_config"]["format"],
        json!({ "type": "json_schema", "schema": responses_schema })
    );
    assert!(upstream["output_config"].get("name").is_none());
    assert!(upstream["output_config"].get("strict").is_none());
}

#[tokio::test]
async fn messages_nonstream_preserves_exact_stop_reason() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "stream_mode": "messages_pause_turn",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "pause" }] }
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let body: Value = serde_json::from_str(&body).expect("messages response JSON");
    assert_eq!(body["stop_reason"], json!("pause_turn"));
}

/// Downstream `/v1/messages` MUST accept `redacted_thinking` content blocks per PM5a and the
/// upstream request MUST re-emit the block with its `data` field preserved verbatim and the
/// block type MUST be `redacted_thinking` (not `thinking`). See DM5.1 case 2.
#[tokio::test]
async fn messages_request_roundtrips_redacted_thinking_block() {
    let ctx = setup().await;
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "redacted_thinking",
                            "data": "redacted_opaque_blob"
                        },
                        { "type": "text", "text": "answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "again" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let redacted = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("redacted_thinking"))
        .expect("redacted_thinking block forwarded unchanged");
    assert_eq!(redacted["data"], "redacted_opaque_blob");
    assert!(
        redacted.get("thinking").is_none(),
        "redacted_thinking blocks must not carry a `thinking` field"
    );
}

/// End-to-end round trip for OpenAI Responses `rs_...` item id preservation through a Messages
/// downstream. Simulates the Claude Code -> monoize -> Responses upstream scenario that
/// originally produced `invalid_encrypted_content: Encrypted content item_id did not match`.
///
/// Flow:
/// 1. Client sends a downstream `/v1/messages` request whose assistant history contains a
///    `thinking` block whose `signature` carries the sigil `mz1.rs_original.<sig>`.
/// 2. monoize decodes the block into `Node::Reasoning { id: Some("rs_original"), encrypted: "<sig>" }`.
/// 3. monoize encodes the URP request to a Responses upstream.
/// 4. The upstream Responses request MUST contain a `reasoning` item whose `id` is exactly
///    `rs_original` - not a freshly synthesized `rs_urp_*` - and whose `encrypted_content` is
///    the stripped original signature, not the sigil string.
#[tokio::test]
async fn messages_item_id_roundtrips_to_responses_upstream_item_id() {
    let ctx = setup().await;
    let sigil = "mz1.rs_original_from_upstream.prior_encrypted_content";
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "first turn" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "prior reasoning",
                            "signature": sigil
                        },
                        { "type": "text", "text": "prior answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "responses");
    let input = upstream["input"].as_array().expect("responses input array");
    let reasoning_item = input
        .iter()
        .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("reasoning"))
        .expect("responses upstream request should contain the replayed reasoning item");
    assert_eq!(
        reasoning_item["id"].as_str(),
        Some("rs_original_from_upstream"),
        "Reasoning item id must be extracted from the signature sigil and forwarded so that `encrypted_content` stays cryptographically bound to the original upstream item id"
    );
    assert_eq!(
        reasoning_item["encrypted_content"].as_str(),
        Some("prior_encrypted_content"),
        "encrypted_content must be the original signature, stripped of the sigil prefix"
    );
}

/// When monoize returns a `/v1/messages` response that embeds reasoning originally produced by
/// a Responses upstream, the downstream Anthropic `thinking.signature` MUST carry the sigil
/// `mz1.<item_id>.<original_signature>`. Claude Code and other Anthropic clients strip unknown
/// content-block fields, so we smuggle the item id inside `signature` instead of attaching a
/// custom field.
#[tokio::test]
async fn messages_response_signature_embeds_item_id_sigil_from_responses_upstream() {
    let ctx = setup().await;
    let (status, body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 2048 },
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "show reasoning" }] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let blocks = v["content"].as_array().cloned().unwrap_or_default();
    let thinking = blocks
        .iter()
        .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
        .expect("downstream response should contain a thinking block");
    let signature = thinking
        .get("signature")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        signature.starts_with("mz2."),
        "thinking.signature must embed a Monoize mz2 envelope so downstream clients echo item/model metadata; got `{signature}`"
    );
}

/// When forwarding an assistant reasoning node to a real Anthropic upstream (a Messages-type
/// provider), monoize MUST strip any sigil prefix from `signature` so that the upstream receives
/// only the opaque original payload. Otherwise Anthropic's own signature validation would reject
/// the sigil-prefixed value.
#[tokio::test]
async fn messages_upstream_request_strips_sigil_from_signature() {
    let ctx = setup().await;
    let sigil = "mz1.rs_original.original_anthropic_signature";
    let (status, _body) = json_post(
        &ctx,
        "/v1/messages",
        json!({
            "model": "gpt-5-mini-msg",
            "max_tokens": 64,
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "prior reasoning",
                            "signature": sigil
                        },
                        { "type": "text", "text": "prior answer" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "continue" }] }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let upstream = last_captured_body(&ctx, "messages");
    let assistant = upstream["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant turn forwarded");
    let thinking_block = assistant["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("thinking"))
        .expect("thinking block forwarded");
    assert_eq!(
        thinking_block["signature"].as_str(),
        Some("original_anthropic_signature"),
        "Messages upstream must receive a clean signature, stripped of monoize's sigil prefix, so Anthropic's signature validation does not reject it"
    );
}

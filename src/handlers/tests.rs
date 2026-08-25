use super::*;
use crate::app::{RuntimeConfig, load_state_with_runtime};
use crate::auth::AuthResult;
use crate::billing_rate_store::DbBillingRateRecord;
use crate::model_registry_store::ModelPricing;
use crate::monoize_routing::{
    CreateMonoizeChannelInput, CreateMonoizeProviderInput, MonoizeModelEntry, MonoizeProviderType,
};
use crate::settings::normalize_pricing_model_key;
use crate::urp;
use crate::users::{
    CompiledModelRedirectRule, ModelRedirectRule, RequestCaptureMode, UserRole,
    compile_model_redirects,
};
use axum::http::StatusCode;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

const GROUP_ROUTING_MODEL: &str = "gpt-group-routing";

fn test_rate(
    id: &str,
    usage_class: &str,
    unit_price: i128,
    context_tier: Option<&str>,
    modality: Option<&str>,
    cache_ttl: Option<&str>,
    match_json: Value,
) -> DbBillingRateRecord {
    DbBillingRateRecord {
        id: id.to_string(),
        source: "test".to_string(),
        pricing_profile: "test".to_string(),
        model_pattern: Some("test-model".to_string()),
        provider_type: Some("responses".to_string()),
        rate_kind: "token".to_string(),
        usage_class: usage_class.to_string(),
        unit: "token".to_string(),
        unit_price_nano_usd: unit_price.to_string(),
        context_tier: context_tier.map(str::to_string),
        service_tier: None,
        modality: modality.map(str::to_string),
        cache_ttl: cache_ttl.map(str::to_string),
        match_json,
        priority: 0,
        enabled: true,
        raw_json: serde_json::json!({}),
        updated_at: chrono::Utc::now(),
    }
}

fn test_meter_rate(
    id: &str,
    usage_class: &str,
    unit: &str,
    unit_price: i128,
    match_json: Value,
) -> DbBillingRateRecord {
    DbBillingRateRecord {
        id: id.to_string(),
        source: "test".to_string(),
        pricing_profile: "test".to_string(),
        model_pattern: Some("test-model".to_string()),
        provider_type: Some("responses".to_string()),
        rate_kind: "meter".to_string(),
        usage_class: usage_class.to_string(),
        unit: unit.to_string(),
        unit_price_nano_usd: unit_price.to_string(),
        context_tier: None,
        service_tier: None,
        modality: None,
        cache_ttl: None,
        match_json,
        priority: 0,
        enabled: true,
        raw_json: serde_json::json!({}),
        updated_at: chrono::Utc::now(),
    }
}

fn test_resolution(rates: Vec<DbBillingRateRecord>) -> BillingRateResolution {
    BillingRateResolution {
        pricing_profile: "test".to_string(),
        pricing_model: "test-model".to_string(),
        rates,
    }
}

fn build_test_auth(effective_groups: Option<Vec<String>>) -> AuthResult {
    build_test_auth_with_role(effective_groups, UserRole::User)
}

fn build_test_auth_with_role(
    effective_groups: Option<Vec<String>>,
    user_role: UserRole,
) -> AuthResult {
    AuthResult {
        tenant_id: "tenant-1".to_string(),
        user_id: None,
        username: None,
        user_role,
        api_key_id: None,
        api_key_name: None,
        internal_source: None,
        max_multiplier: None,
        transforms: Vec::new(),
        model_redirects: Vec::new(),
        effective_groups,
        model_limits_enabled: false,
        model_limits: Vec::new(),
        ip_whitelist: Vec::new(),
        sub_account_enabled: false,
        sub_account_balance_nano: "0".to_string(),
        reasoning_envelope_enabled: true,
        request_capture_mode: RequestCaptureMode::Off,
    }
}

fn build_test_urp_request(model: &str) -> urp::UrpRequest {
    urp::UrpRequest {
        model: model.to_string(),
        input: vec![urp::Node::Text {
            id: None,
            role: urp::OrdinaryRole::User,
            content: "hello".to_string(),
            phase: None,
            extra_body: HashMap::new(),
        }],
        stream: Some(false),
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        reasoning: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        stop: None,
        verbosity: None,
        response_format: None,
        user: None,
        extra_body: HashMap::new(),
    }
}

#[test]
fn provider_extra_filter_retains_internal_state_until_same_chat_encoding() {
    let mut req = urp::decode::openai_chat::decode_request(&serde_json::json!({
        "model": "gpt-4-0613",
        "messages": [{ "role": "user", "content": "hello" }],
        "functions": [{
            "name": "lookup",
            "parameters": { "type": "object" }
        }],
        "function_call": { "name": "lookup", "x_choice": "kept" },
        "unlisted_provider_field": "drop"
    }))
    .expect("decode deprecated Chat controls");

    filter_extra_body_for_provider(&mut req, ProviderType::ChatCompletion, &None);

    assert!(
        req.extra_body
            .contains_key(urp::CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY)
    );
    assert!(!req.extra_body.contains_key("unlisted_provider_field"));

    let wire = urp::encode::openai_chat::encode_request(&req, "gpt-4-0613");
    assert_eq!(
        wire["function_call"],
        serde_json::json!({ "name": "lookup", "x_choice": "kept" })
    );
    assert_eq!(wire["functions"][0]["name"], serde_json::json!("lookup"));
    assert!(wire.get("tool_choice").is_none());
    assert!(wire.get("tools").is_none());
    assert!(!wire.to_string().contains("_monoize_"));
}

fn build_test_routing_request(model: &str) -> UrpRequest {
    UrpRequest {
        model: model.to_string(),
        max_multiplier: None,
        server_tool_usage_classes: Vec::new(),
        affinity_explicit: None,
        affinity_prefix_hash: crate::handlers::helpers::short_xxh3_hex(model),
    }
}

fn build_model_redirect_rule(pattern: &str, replace: &str) -> CompiledModelRedirectRule {
    compile_model_redirects(&[ModelRedirectRule {
        pattern: pattern.to_string(),
        replace: replace.to_string(),
    }])
    .expect("model redirect compiles")
    .pop()
    .expect("compiled rule exists")
}

#[test]
fn strip_orphaned_tool_calls_keeps_only_closed_stateless_pairs() {
    let mut req = urp::UrpRequest {
        model: "gpt-5.5".to_string(),
        input: vec![
            urp::Node::Text {
                id: None,
                role: urp::OrdinaryRole::User,
                content: "start".to_string(),
                phase: None,
                extra_body: HashMap::new(),
            },
            urp::Node::ToolCall {
                id: Some("fc_answered".to_string()),
                tool_type: urp::ToolCallType::Function,
                call_id: "call_answered".to_string(),
                name: "tool".to_string(),
                arguments: "{}".to_string(),
                extra_body: HashMap::new(),
            },
            urp::Node::ToolCall {
                id: Some("fc_unanswered".to_string()),
                tool_type: urp::ToolCallType::Function,
                call_id: "call_unanswered".to_string(),
                name: "tool".to_string(),
                arguments: "{}".to_string(),
                extra_body: HashMap::new(),
            },
            urp::Node::ToolResult {
                id: None,
                tool_type: urp::ToolCallType::Function,
                call_id: "call_answered".to_string(),
                is_error: false,
                content: vec![urp::ToolResultContent::Text {
                    text: "ok".to_string(),
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            },
            urp::Node::ToolResult {
                id: None,
                tool_type: urp::ToolCallType::Function,
                call_id: "call_missing".to_string(),
                is_error: false,
                content: vec![urp::ToolResultContent::Text {
                    text: "orphan".to_string(),
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            },
            urp::Node::Text {
                id: None,
                role: urp::OrdinaryRole::User,
                content: "interrupt".to_string(),
                phase: None,
                extra_body: HashMap::new(),
            },
        ],
        stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        reasoning: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        stop: None,
        verbosity: None,
        response_format: None,
        user: None,
        extra_body: HashMap::new(),
    };

    strip_orphaned_tool_calls(&mut req);

    let call_ids: Vec<&str> = req
        .input
        .iter()
        .filter_map(|node| match node {
            urp::Node::ToolCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    let result_ids: Vec<&str> = req
        .input
        .iter()
        .filter_map(|node| match node {
            urp::Node::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(call_ids, vec!["call_answered"]);
    assert_eq!(result_ids, vec!["call_answered"]);
    assert!(
        req.input
            .iter()
            .any(|node| matches!(node, urp::Node::Text { content, .. } if content == "interrupt"))
    );
}

#[test]
fn responses_tool_replay_preserves_plaintext_raw_cot_reasoning() {
    let req = urp::UrpRequest {
        model: "gpt-5.5".to_string(),
        input: vec![
            urp::Node::Reasoning {
                id: Some("rs_plain".to_string()),
                content: Some("plain summary".to_string()),
                encrypted: None,
                summary: Some("plain summary".to_string()),
                source: None,
                extra_body: HashMap::new(),
            },
            urp::Node::ToolCall {
                id: Some("fc_answered".to_string()),
                tool_type: urp::ToolCallType::Function,
                call_id: "call_answered".to_string(),
                name: "tool".to_string(),
                arguments: "{}".to_string(),
                extra_body: HashMap::new(),
            },
            urp::Node::ToolResult {
                id: None,
                tool_type: urp::ToolCallType::Function,
                call_id: "call_answered".to_string(),
                is_error: false,
                content: vec![urp::ToolResultContent::Text {
                    text: "ok".to_string(),
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            },
            urp::Node::Reasoning {
                id: Some("rs_encrypted".to_string()),
                content: Some("kept".to_string()),
                encrypted: Some(serde_json::json!("sig_kept")),
                summary: Some("kept".to_string()),
                source: None,
                extra_body: HashMap::new(),
            },
        ],
        stream: None,
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        reasoning: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        stop: None,
        verbosity: None,
        response_format: None,
        user: None,
        extra_body: HashMap::new(),
    };

    let encoded = urp::encode::openai_responses::encode_request(&req, "gpt-5.5");
    let input = encoded["input"].as_array().expect("Responses input array");
    let plaintext = input
        .iter()
        .find(|item| item.get("id") == Some(&serde_json::json!("rs_plain")))
        .expect("plaintext RawCoT reasoning item");
    assert_eq!(
        plaintext["content"],
        serde_json::json!([{
            "type": "reasoning_text",
            "text": "plain summary"
        }])
    );
    assert!(
        input
            .iter()
            .any(|item| item.get("id") == Some(&serde_json::json!("rs_encrypted")))
    );
}

async fn seed_group_routing_provider(
    state: &AppState,
    name: &str,
    circuit_breaker_enabled: bool,
    group_ids: Vec<String>,
    channels: Vec<CreateMonoizeChannelInput>,
) {
    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: name.to_string(),
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            enabled: true,
            priority: Some(0),
            group_ids,
            channels,
        })
        .await
        .expect("provider created");
}

async fn seed_model_pricing(state: &AppState, model: &str) {
    state
        .model_registry_store
        .upsert_model_metadata(
            model,
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some(Some("openai".to_string())),
                mode: Some(Some("chat".to_string())),
                input_cost_per_token_nano: Some(Some("1000".to_string())),
                output_cost_per_token_nano: Some(Some("1000".to_string())),
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("pricing seeded");
}

#[tokio::test]
async fn routing_uses_channel_model_multiplier_and_redirect_per_attempt() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    seed_model_pricing(&state, "channel-owned-model").await;

    let channel = |id: &str, multiplier: &str, redirect: &str| CreateMonoizeChannelInput {
        id: Some(id.to_string()),
        name: id.to_string(),
        provider_type: MonoizeProviderType::Responses,
        base_url: format!("https://{id}.example.com"),
        api_key: Some("secret".to_string()),
        weight: 1,
        enabled: true,
        passive_failure_count_threshold_override: None,
        passive_cooldown_seconds_override: None,
        passive_window_seconds_override: None,
        passive_rate_limit_cooldown_seconds_override: None,
        models: std::collections::HashMap::from([(
            "channel-owned-model".to_string(),
            MonoizeModelEntry {
                redirect: Some(redirect.to_string()),
                multiplier: multiplier.parse().unwrap(),
            },
        )]),
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        affinity_enabled_override: None,
        affinity_idle_ttl_seconds_override: None,
        affinity_failback_mode_override: None,
        affinity_failback_delay_seconds_override: None,

        proxy_url: None,
        extra_headers: None,
        session_affinity_auto: None,
    };

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "channel-owned-models".to_string(),
            channels: vec![
                channel("cheap", "1", "cheap-upstream"),
                channel("expensive", "2", "expensive-upstream"),
            ],
            max_retries: -1,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: false,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
        })
        .await
        .expect("provider created");

    let mut request = build_test_routing_request("channel-owned-model");
    request.max_multiplier = Some("1.5".parse().unwrap());
    let attempts = build_monoize_attempts(&state, &request, &build_test_auth(None))
        .await
        .expect("routing succeeds");

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].channel_id, "cheap");
    assert_eq!(attempts[0].upstream_model, "cheap-upstream");
    assert_eq!(attempts[0].model_multiplier, Multiplier::ONE);
}

fn attempt_channel_ids(attempts: &[MonoizeAttempt]) -> BTreeSet<&str> {
    attempts
        .iter()
        .map(|attempt| attempt.channel_id.as_str())
        .collect()
}

#[test]
fn calculate_charge_nano_uses_model_price_and_multiplier() {
    let usage = urp::Usage {
        input_tokens: 15,
        output_tokens: 5,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    };
    let pricing = ModelPricing {
        input_cost_per_token_nano: 2500,
        output_cost_per_token_nano: 10000,
        cache_read_input_cost_per_token_nano: None,
        cache_creation_input_cost_per_token_nano: None,
        output_cost_per_reasoning_token_nano: None,
    };

    let charged = calculate_charge_nano(
        &usage,
        &pricing,
        Multiplier::parse("1.234567891").expect("valid multiplier"),
    );

    assert_eq!(charged, Some(108_024));
}

#[test]
fn calculate_charge_nano_handles_cached_and_reasoning_tokens() {
    let usage = urp::Usage {
        input_tokens: 100,
        output_tokens: 80,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 60,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 0,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: Some(urp::OutputDetails {
            standard_tokens: 0,
            reasoning_tokens: 30,
            accepted_prediction_tokens: 0,
            rejected_prediction_tokens: 0,
            modality_breakdown: None,
        }),
        extra_body: HashMap::new(),
    };
    let pricing = ModelPricing {
        input_cost_per_token_nano: 1000,
        output_cost_per_token_nano: 2000,
        cache_read_input_cost_per_token_nano: Some(100),
        cache_creation_input_cost_per_token_nano: None,
        output_cost_per_reasoning_token_nano: Some(3000),
    };

    let charged = calculate_charge_nano(&usage, &pricing, Multiplier::ONE);

    assert_eq!(charged, Some(236_000));
}

#[test]
fn calculate_charge_nano_messages_treats_cache_creation_as_disjoint_bucket() {
    // Post-decode normalization: Anthropic wire input_tokens=100 + cache_creation=40
    // becomes internal input_tokens=140. Billing uniformly subtracts cache buckets.
    // See user-billing-and-model-metadata.spec.md § 5 C3-ii, C3a.
    let usage = urp::Usage {
        input_tokens: 140,
        output_tokens: 20,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 0,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 40,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };
    let pricing = ModelPricing {
        input_cost_per_token_nano: 1000,
        output_cost_per_token_nano: 2000,
        cache_read_input_cost_per_token_nano: None,
        cache_creation_input_cost_per_token_nano: Some(250),
        output_cost_per_reasoning_token_nano: None,
    };

    let charged = calculate_charge_nano(&usage, &pricing, Multiplier::ONE);

    assert_eq!(charged, Some(150_000));
}

#[test]
fn calculate_charge_nano_responses_excludes_cache_creation_from_inclusive_input_total() {
    let usage = urp::Usage {
        input_tokens: 100,
        output_tokens: 20,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 0,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 40,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };
    let pricing = ModelPricing {
        input_cost_per_token_nano: 1000,
        output_cost_per_token_nano: 2000,
        cache_read_input_cost_per_token_nano: Some(100),
        cache_creation_input_cost_per_token_nano: Some(250),
        output_cost_per_reasoning_token_nano: None,
    };

    let charged = calculate_charge_nano(&usage, &pricing, Multiplier::ONE);

    assert_eq!(charged, Some(110_000));
}

#[test]
fn calculate_charge_nano_responses_avoids_double_count_when_cache_read_and_creation_are_both_present()
 {
    let usage = urp::Usage {
        input_tokens: 100,
        output_tokens: 10,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 30,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 20,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };
    let pricing = ModelPricing {
        input_cost_per_token_nano: 1000,
        output_cost_per_token_nano: 2000,
        cache_read_input_cost_per_token_nano: Some(100),
        cache_creation_input_cost_per_token_nano: Some(250),
        output_cost_per_reasoning_token_nano: None,
    };

    let charged = calculate_charge_nano(&usage, &pricing, Multiplier::ONE);

    assert_eq!(charged, Some(78_000));
}

#[test]
fn rate_matrix_selects_short_vs_long_context_tier() {
    let threshold = serde_json::json!({ "context_threshold_tokens": 128000 });
    let resolution = test_resolution(vec![
        test_rate(
            "short-input",
            "input_uncached",
            1,
            Some("short"),
            None,
            None,
            threshold.clone(),
        ),
        test_rate(
            "short-output",
            "output",
            2,
            Some("short"),
            None,
            None,
            threshold.clone(),
        ),
        test_rate(
            "long-input",
            "input_uncached",
            10,
            Some("long"),
            None,
            None,
            threshold.clone(),
        ),
        test_rate(
            "long-output",
            "output",
            20,
            Some("long"),
            None,
            None,
            threshold,
        ),
    ]);
    assert!(
        billing_rate_matrix_allows_request(&resolution, &Vec::new())
            .expect("tiered matrix has threshold")
    );
    let usage = urp::Usage {
        input_tokens: 128001,
        output_tokens: 10,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    };

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect("charge succeeds");

    assert_eq!(components.context_tier.as_deref(), Some("long"));
    assert_eq!(components.base_charge, 1_280_210);
}

#[test]
fn rate_matrix_bills_anthropic_cache_ttl_split_and_read() {
    let resolution = test_resolution(vec![
        test_rate(
            "input",
            "input_uncached",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "read",
            "cache_read",
            2,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "write-5m",
            "cache_write_5m",
            3,
            None,
            None,
            Some("5m"),
            serde_json::json!({}),
        ),
        test_rate(
            "write-1h",
            "cache_write_1h",
            4,
            None,
            None,
            Some("1h"),
            serde_json::json!({}),
        ),
        test_rate(
            "output",
            "output",
            5,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
    ]);
    let usage = urp::Usage {
        input_tokens: 1000,
        output_tokens: 10,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 100,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 300,
            cache_creation_5m_tokens: 200,
            cache_creation_1h_tokens: 100,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect("charge succeeds");

    assert_eq!(components.base_charge, 1850);
    assert_eq!(components.token_line_items.len(), 5);
}

#[test]
fn rate_matrix_rejects_aggregate_cache_creation_without_ttl_split() {
    let resolution = test_resolution(vec![
        test_rate(
            "input",
            "input_uncached",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "write-5m",
            "cache_write_5m",
            3,
            None,
            None,
            Some("5m"),
            serde_json::json!({}),
        ),
        test_rate(
            "write-1h",
            "cache_write_1h",
            4,
            None,
            None,
            Some("1h"),
            serde_json::json!({}),
        ),
        test_rate(
            "output",
            "output",
            5,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
    ]);
    let usage = urp::Usage {
        input_tokens: 1000,
        output_tokens: 10,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 0,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 300,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };

    let err = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect_err("aggregate cache write must not be guessed");

    assert!(err.contains("requires 5m/1h split"));
}

#[test]
fn rate_matrix_uses_dimensionless_defaults_for_cache_and_missing_modality_breakdown() {
    let resolution = test_resolution(vec![
        test_rate(
            "input-default",
            "input_uncached",
            1,
            None,
            None,
            None,
            json!({}),
        ),
        test_rate(
            "input-image",
            "input_uncached",
            9,
            None,
            Some("image"),
            None,
            json!({}),
        ),
        test_rate("output-default", "output", 2, None, None, None, json!({})),
        test_rate(
            "output-image",
            "output",
            9,
            None,
            Some("image"),
            None,
            json!({}),
        ),
    ]);
    let usage = urp::Usage {
        input_tokens: 100,
        output_tokens: 10,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 20,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 30,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect("dimensionless fallback should settle every token bucket");

    assert_eq!(components.base_charge, 120);
    assert_eq!(
        components
            .token_line_items
            .iter()
            .filter(|line| line["rate_id"] == json!("input-default"))
            .count(),
        3
    );
}

#[test]
fn rate_matrix_preflight_rejects_invalid_prices_and_missing_dimensionless_defaults() {
    let mut negative = test_rate(
        "negative",
        "input_uncached",
        -1,
        None,
        None,
        None,
        json!({}),
    );
    negative.priority = 100;
    let invalid_price = test_resolution(vec![
        negative,
        test_rate("input", "input_uncached", 1, None, None, None, json!({})),
        test_rate("output", "output", 1, None, None, None, json!({})),
    ]);
    assert!(
        billing_rate_matrix_allows_request(&invalid_price, &Vec::new())
            .expect_err("negative candidate price must fail preflight")
            .contains("negative unit_price")
    );

    let modality_only = test_resolution(vec![
        test_rate(
            "input-image",
            "input_uncached",
            1,
            None,
            Some("image"),
            None,
            json!({}),
        ),
        test_rate(
            "output-image",
            "output",
            1,
            None,
            Some("image"),
            None,
            json!({}),
        ),
    ]);
    assert_eq!(
        billing_rate_matrix_allows_request(&modality_only, &Vec::new()),
        Ok(false)
    );
}

#[test]
fn rate_matrix_bills_gpt_image_2_modality_token_lines() {
    let resolution = test_resolution(vec![
        test_rate(
            "input-text",
            "input_uncached",
            1,
            None,
            Some("text"),
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "input-image",
            "input_uncached",
            2,
            None,
            Some("image"),
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "cache-text",
            "cache_read",
            4,
            None,
            Some("text"),
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "cache-image",
            "cache_read",
            5,
            None,
            Some("image"),
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "output-image",
            "output",
            3,
            None,
            Some("image"),
            None,
            serde_json::json!({}),
        ),
    ]);
    let usage = urp::Usage {
        input_tokens: 160,
        output_tokens: 20,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 10,
            cache_read_modality_breakdown: Some(urp::ModalityBreakdown {
                text_tokens: Some(6),
                image_tokens: Some(4),
                audio_tokens: None,
                video_tokens: None,
                document_tokens: None,
            }),
            cache_creation_tokens: 0,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: Some(urp::ModalityBreakdown {
                text_tokens: Some(106),
                image_tokens: Some(54),
                audio_tokens: None,
                video_tokens: None,
                document_tokens: None,
            }),
        }),
        output_details: Some(urp::OutputDetails {
            standard_tokens: 0,
            reasoning_tokens: 0,
            accepted_prediction_tokens: 0,
            rejected_prediction_tokens: 0,
            modality_breakdown: Some(urp::ModalityBreakdown {
                text_tokens: None,
                image_tokens: Some(20),
                audio_tokens: None,
                video_tokens: None,
                document_tokens: None,
            }),
        }),
        extra_body: HashMap::new(),
    };

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect("charge succeeds");

    assert_eq!(components.base_charge, 304);
    assert_eq!(components.token_line_items.len(), 5);
}

#[test]
fn rate_matrix_supports_input_cached_usage_class_alias() {
    let resolution = test_resolution(vec![
        test_rate(
            "input",
            "input_uncached",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "cached",
            "input_cached",
            2,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "output",
            "output",
            3,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
    ]);
    let usage = urp::Usage {
        input_tokens: 100,
        output_tokens: 10,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 40,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 0,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 0,
            modality_breakdown: None,
        }),
        output_details: None,
        extra_body: HashMap::new(),
    };

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect("charge succeeds");

    assert_eq!(components.base_charge, 170);
    assert_eq!(
        components.token_line_items[1]["usage_class"].as_str(),
        Some("input_cached")
    );
}

#[test]
fn rate_matrix_does_not_double_add_inclusive_tool_prompt_or_reasoning_details() {
    let resolution = test_resolution(vec![
        test_rate(
            "input",
            "input_uncached",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "output",
            "output",
            2,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "reasoning",
            "reasoning_output",
            3,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
    ]);
    let usage = urp::Usage {
        input_tokens: 130,
        output_tokens: 120,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 0,
            cache_read_modality_breakdown: None,
            cache_creation_tokens: 0,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: 30,
            modality_breakdown: None,
        }),
        output_details: Some(urp::OutputDetails {
            standard_tokens: 0,
            reasoning_tokens: 20,
            accepted_prediction_tokens: 0,
            rejected_prediction_tokens: 0,
            modality_breakdown: None,
        }),
        extra_body: HashMap::new(),
    };

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &resolution,
        Multiplier::ONE,
        &Vec::new(),
    )
    .expect("charge succeeds");

    assert_eq!(components.base_charge, 390);
}

#[test]
fn rate_matrix_counts_call_meter_from_decoded_native_events_and_requires_duration_usage() {
    let call_resolution = test_resolution(vec![
        test_rate(
            "input",
            "input_uncached",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "output",
            "output",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_meter_rate("web", "web_search", "call", 100, serde_json::json!({})),
    ]);
    let usage = urp::Usage {
        input_tokens: 1,
        output_tokens: 1,
        input_details: None,
        output_details: None,
        extra_body: HashMap::new(),
    };
    let output = vec![urp::Node::ProviderItem {
        id: None,
        origin_protocol: urp::ProviderProtocol::Responses,
        role: urp::OrdinaryRole::Assistant,
        item_type: "web_search_call".to_string(),
        body: serde_json::json!({}),
        extra_body: HashMap::new(),
    }];

    let call_components = calculate_rate_matrix_charge_components(
        &usage,
        Some(&output),
        None,
        &call_resolution,
        Multiplier::ONE,
        &["web_search".to_string()],
    )
    .expect("decoded call is billable");

    assert_eq!(call_components.base_charge, 102);

    let duration_resolution = test_resolution(vec![
        test_rate(
            "input",
            "input_uncached",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_rate(
            "output",
            "output",
            1,
            None,
            None,
            None,
            serde_json::json!({}),
        ),
        test_meter_rate(
            "code-duration",
            "code_interpreter_duration",
            "billed_minute",
            1_000,
            serde_json::json!({ "requires_authoritative_usage": true }),
        ),
    ]);
    let err = calculate_rate_matrix_charge_components(
        &usage,
        None,
        None,
        &duration_resolution,
        Multiplier::ONE,
        &["code_interpreter_duration".to_string()],
    )
    .expect_err("duration meter must require authoritative usage");

    assert!(err.contains("authoritative usage required"));
}

#[test]
fn rate_matrix_uses_only_first_dimension_matching_meter_row_per_usage_class() {
    let mut wrong_service =
        test_meter_rate("wrong-service", "web_search", "call", 10_000, json!({}));
    wrong_service.service_tier = Some("priority".to_string());
    let resolution = test_resolution(vec![
        test_rate("input", "input_uncached", 1, None, None, None, json!({})),
        test_rate("output", "output", 1, None, None, None, json!({})),
        wrong_service,
        test_meter_rate("selected", "web_search", "call", 100, json!({})),
        test_meter_rate("duplicate", "web_search", "call", 500, json!({})),
    ]);
    let usage = urp::Usage {
        input_tokens: 1,
        output_tokens: 1,
        input_details: None,
        output_details: None,
        extra_body: HashMap::from([("service_tier".to_string(), json!("batch"))]),
    };
    let output = vec![urp::Node::ProviderItem {
        id: None,
        origin_protocol: urp::ProviderProtocol::Responses,
        role: urp::OrdinaryRole::Assistant,
        item_type: "web_search_call".to_string(),
        body: json!({}),
        extra_body: HashMap::new(),
    }];

    let components = calculate_rate_matrix_charge_components(
        &usage,
        Some(&output),
        None,
        &resolution,
        Multiplier::ONE,
        &["web_search".to_string()],
    )
    .expect("matching meter row should be selected once");

    assert_eq!(components.base_charge, 102);
    assert_eq!(components.meter_line_items.len(), 1);
    assert_eq!(components.meter_line_items[0]["rate_id"], json!("selected"));
}

#[test]
fn rate_matrix_uses_actual_response_service_tier_and_requires_exact_rates() {
    let default_rates = vec![
        test_rate(
            "input-default",
            "input_uncached",
            1,
            None,
            None,
            None,
            json!({}),
        ),
        test_rate("output-default", "output", 2, None, None, None, json!({})),
    ];
    let usage = urp::Usage {
        input_tokens: 1,
        output_tokens: 1,
        input_details: None,
        output_details: None,
        extra_body: HashMap::from([("service_tier".to_string(), json!("default"))]),
    };

    let err = calculate_rate_matrix_charge_components(
        &usage,
        None,
        Some("priority"),
        &test_resolution(default_rates.clone()),
        Multiplier::ONE,
        &[],
    )
    .expect_err("a non-default response tier must not use default rates");
    assert!(err.contains("service_tier=\"priority\""), "{err}");

    let mut priority_input = test_rate(
        "input-priority",
        "input_uncached",
        20,
        None,
        None,
        None,
        json!({}),
    );
    priority_input.service_tier = Some("priority".to_string());
    let mut priority_output =
        test_rate("output-priority", "output", 30, None, None, None, json!({}));
    priority_output.service_tier = Some("priority".to_string());
    let mut rates = default_rates;
    rates.extend([priority_input, priority_output]);

    let components = calculate_rate_matrix_charge_components(
        &usage,
        None,
        Some("priority"),
        &test_resolution(rates),
        Multiplier::ONE,
        &[],
    )
    .expect("the exact response-tier rates are billable");
    assert_eq!(components.service_tier.as_deref(), Some("priority"));
    assert_eq!(components.base_charge, 50);
    assert_eq!(
        components.token_line_items[0]["rate_id"],
        json!("input-priority")
    );
    assert_eq!(
        components.token_line_items[1]["rate_id"],
        json!("output-priority")
    );
}

#[test]
fn scale_charge_uses_exact_decimal_and_truncates_toward_zero() {
    let base = 100_000_000i128;
    let charged = scale_charge_with_multiplier(
        base,
        Multiplier::parse("1.000000009").expect("valid multiplier"),
    );
    assert_eq!(charged, Some(100_000_000));
}

#[test]
fn scale_charge_avoids_overflow_when_final_value_is_representable() {
    let base = i128::MAX / 2;
    let charged =
        scale_charge_with_multiplier(base, Multiplier::parse("1.1").expect("valid multiplier"));

    assert_eq!(charged, base.checked_add(base / 10));
}

#[test]
fn normalize_pricing_model_key_strips_recognized_reasoning_suffix() {
    let suffix_map = std::collections::HashMap::from([
        ("-thinking".to_string(), "high".to_string()),
        ("-nothinking".to_string(), "none".to_string()),
    ]);

    assert_eq!(
        normalize_pricing_model_key("gpt-5-mini-thinking", &suffix_map),
        "gpt-5-mini"
    );
    assert_eq!(
        normalize_pricing_model_key("gpt-5-mini-high", &suffix_map),
        "gpt-5-mini"
    );
}

#[tokio::test]
async fn resolve_model_suffix_preserves_reasoning_effort_on_attempt_base_request() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "OpenAI".to_string(),
            max_retries: 0,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "primary".to_string(),
                provider_type: MonoizeProviderType::Responses,
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: std::collections::HashMap::from([(
                    "gpt-5-mini".to_string(),
                    MonoizeModelEntry {
                        redirect: None,
                        multiplier: Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
        })
        .await
        .expect("provider created");

    let mut req = build_test_urp_request("gpt-5-mini-thinking");
    resolve_model_suffix(&state, &mut req).await.unwrap();
    let original_req = req.clone();

    assert_eq!(original_req.model, "gpt-5-mini");
    assert_eq!(
        original_req
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.effort.as_deref()),
        Some("high")
    );

    let mut explicitly_configured_req = build_test_urp_request("gpt-5-mini-thinking");
    explicitly_configured_req.reasoning = Some(urp::ReasoningConfig {
        effort: Some("low".to_string()),
        extra_body: std::collections::HashMap::new(),
    });
    resolve_model_suffix(&state, &mut explicitly_configured_req)
        .await
        .unwrap();

    assert_eq!(explicitly_configured_req.model, "gpt-5-mini");
    assert_eq!(
        explicitly_configured_req
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.effort.as_deref()),
        Some("low")
    );
}

#[test]
fn resolve_upstream_model_prefers_non_empty_redirect() {
    let entry = MonoizeModelEntry {
        redirect: Some("  gpt-5-target  ".to_string()),
        multiplier: Multiplier::ONE,
    };
    assert_eq!(
        resolve_upstream_model("gpt-5-logical", &entry),
        "gpt-5-target".to_string()
    );
}

#[test]
fn resolve_upstream_model_falls_back_to_requested_when_redirect_blank() {
    let entry = MonoizeModelEntry {
        redirect: Some("   ".to_string()),
        multiplier: Multiplier::ONE,
    };
    assert_eq!(
        resolve_upstream_model("gpt-5-logical", &entry),
        "gpt-5-logical".to_string()
    );
}

#[tokio::test]
async fn build_monoize_attempts_rejects_unpriced_models_before_forwarding() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "OpenAI".to_string(),
            max_retries: 0,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "primary".to_string(),
                provider_type: MonoizeProviderType::Responses,
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: std::collections::HashMap::from([(
                    "gpt-unpriced".to_string(),
                    MonoizeModelEntry {
                        redirect: Some("gpt-unpriced-upstream".to_string()),
                        multiplier: Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
        })
        .await
        .expect("provider created");

    let req = build_test_routing_request("gpt-unpriced");
    let auth = build_test_auth(None);
    let err = build_monoize_attempts(&state, &req, &auth)
        .await
        .expect_err("must reject unpriced model");

    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert_eq!(err.code, "model_pricing_required");
    assert!(err.message.contains("gpt-unpriced-upstream"));
}

#[tokio::test]
async fn build_monoize_attempts_rejects_admin_unpriced_models_without_pricing() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "OpenAI".to_string(),
            max_retries: 0,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "primary".to_string(),
                provider_type: MonoizeProviderType::Responses,
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: std::collections::HashMap::from([(
                    "gpt-unpriced".to_string(),
                    MonoizeModelEntry {
                        redirect: Some("gpt-unpriced-upstream".to_string()),
                        multiplier: Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
        })
        .await
        .expect("provider created");

    let req = build_test_routing_request("gpt-unpriced");
    let auth = build_test_auth_with_role(None, UserRole::Admin);
    let err = build_monoize_attempts(&state, &req, &auth)
        .await
        .expect_err("admin unpriced request must be rejected");

    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert_eq!(err.code, "model_pricing_required");
}

#[tokio::test]
async fn build_monoize_attempts_rejects_admin_missing_server_tool_meter_rate() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "OpenAI".to_string(),
            max_retries: 0,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "primary".to_string(),
                provider_type: MonoizeProviderType::Responses,
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: std::collections::HashMap::from([(
                    "gpt-priced".to_string(),
                    MonoizeModelEntry {
                        redirect: None,
                        multiplier: Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
        })
        .await
        .expect("provider created");
    seed_model_pricing(&state, "gpt-priced").await;

    let mut req = build_test_routing_request("gpt-priced");
    req.server_tool_usage_classes = vec!["web_search".to_string()];
    let auth = build_test_auth_with_role(None, UserRole::Admin);
    let err = build_monoize_attempts(&state, &req, &auth)
        .await
        .expect_err("missing meter rate must reject admin");

    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert_eq!(err.code, "model_pricing_required");
    assert!(err.message.contains("meter rate required"));
}

#[tokio::test]
async fn build_monoize_attempts_accepts_redirected_model_when_logical_fallback_is_priced() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "OpenAI".to_string(),
            max_retries: 0,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "primary".to_string(),
                provider_type: MonoizeProviderType::Responses,
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: std::collections::HashMap::from([(
                    "gpt-fallback-src".to_string(),
                    MonoizeModelEntry {
                        redirect: Some("gpt-fallback-dest".to_string()),
                        multiplier: Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
        })
        .await
        .expect("provider created");

    state
        .model_registry_store
        .upsert_model_metadata(
            "gpt-fallback-src",
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some(Some("openai".to_string())),
                mode: Some(Some("chat".to_string())),
                input_cost_per_token_nano: Some(Some("1000".to_string())),
                output_cost_per_token_nano: Some(Some("1000".to_string())),
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("logical pricing seeded");

    state
        .model_registry_store
        .upsert_model_metadata(
            "gpt-fallback-dest",
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some(Some("openai".to_string())),
                mode: Some(Some("chat".to_string())),
                input_cost_per_token_nano: Some(Some("500".to_string())),
                output_cost_per_token_nano: None,
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("partial upstream pricing seeded");

    let resolution = resolve_billing_rate_matrix(
        &state,
        "gpt-fallback-dest",
        "gpt-fallback-src",
        ProviderType::Responses,
    )
    .await
    .expect("pricing lookup")
    .expect("logical fallback pricing");
    assert_eq!(resolution.pricing_model, "gpt-fallback-src");

    let req = build_test_routing_request("gpt-fallback-src");
    let auth = build_test_auth(None);
    let attempts = build_monoize_attempts(&state, &req, &auth)
        .await
        .expect("fallback-priced model should be allowed");

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].upstream_model, "gpt-fallback-dest");
}

#[tokio::test]
async fn build_monoize_attempts_uses_metadata_pricing_profile_fallback() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    state
        .monoize_store
        .create_provider(CreateMonoizeProviderInput {
            name: "Gateway".to_string(),
            max_retries: 0,
            channel_max_retries: 0,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: true,
            per_model_circuit_break: false,
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: None,
            group_ids: Vec::new(),
            enabled: true,
            priority: Some(0),
            channels: vec![CreateMonoizeChannelInput {
                id: None,
                name: "primary".to_string(),
                provider_type: MonoizeProviderType::ChatCompletion,
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
                models: std::collections::HashMap::from([(
                    "claude-sonnet-4.6".to_string(),
                    MonoizeModelEntry {
                        redirect: None,
                        multiplier: Multiplier::ONE,
                    },
                )]),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                affinity_enabled_override: None,
                affinity_idle_ttl_seconds_override: None,
                affinity_failback_mode_override: None,
                affinity_failback_delay_seconds_override: None,

                proxy_url: None,
                extra_headers: None,
                session_affinity_auto: None,
            }],
        })
        .await
        .expect("provider created");

    state
        .model_registry_store
        .upsert_model_metadata(
            "claude-sonnet-4.6",
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some(Some("zenmux".to_string())),
                mode: Some(Some("chat".to_string())),
                input_cost_per_token_nano: Some(Some("3000".to_string())),
                output_cost_per_token_nano: Some(Some("15000".to_string())),
                cache_read_input_cost_per_token_nano: None,
                cache_creation_input_cost_per_token_nano: None,
                output_cost_per_reasoning_token_nano: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_tokens: None,
            },
        )
        .await
        .expect("metadata pricing seeded");

    let req = build_test_routing_request("claude-sonnet-4.6");
    let auth = build_test_auth(None);
    let attempts = build_monoize_attempts(&state, &req, &auth)
        .await
        .expect("metadata-profile fallback should be allowed");

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].upstream_model, "claude-sonnet-4.6");
}

#[tokio::test]
async fn build_monoize_attempts_filters_providers_by_effective_groups_before_health_logic() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let default_group_id = state
        .user_store
        .default_group_id()
        .await
        .expect("default group exists");
    let team_a_group = state
        .user_store
        .create_group(crate::users::CreateGroupInput {
            name: "team-a".to_string(),
            description: String::new(),
            user_selectable: true,
            sort_order: 1,
        })
        .await
        .expect("team-a group created");
    let team_b_group = state
        .user_store
        .create_group(crate::users::CreateGroupInput {
            name: "team-b".to_string(),
            description: String::new(),
            user_selectable: true,
            sort_order: 2,
        })
        .await
        .expect("team-b group created");

    // GR-I2: an empty selection binds the provider to the default group.
    seed_group_routing_provider(
        &state,
        "public-provider",
        false,
        Vec::new(),
        vec![CreateMonoizeChannelInput {
            id: Some("public".to_string()),
            name: "public".to_string(),
            provider_type: MonoizeProviderType::Responses,
            base_url: "https://public.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
            models: std::collections::HashMap::from([(
                GROUP_ROUTING_MODEL.to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: Multiplier::ONE,
                },
            )]),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            affinity_enabled_override: None,
            affinity_idle_ttl_seconds_override: None,
            affinity_failback_mode_override: None,
            affinity_failback_delay_seconds_override: None,

            proxy_url: None,
            extra_headers: None,
            session_affinity_auto: None,
        }],
    )
    .await;
    seed_group_routing_provider(
        &state,
        "team-a-provider",
        false,
        vec![team_a_group.id.clone()],
        vec![CreateMonoizeChannelInput {
            id: Some("team-a".to_string()),
            name: "team-a".to_string(),
            provider_type: MonoizeProviderType::Responses,
            base_url: "https://team-a.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
            models: std::collections::HashMap::from([(
                GROUP_ROUTING_MODEL.to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: Multiplier::ONE,
                },
            )]),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            affinity_enabled_override: None,
            affinity_idle_ttl_seconds_override: None,
            affinity_failback_mode_override: None,
            affinity_failback_delay_seconds_override: None,

            proxy_url: None,
            extra_headers: None,
            session_affinity_auto: None,
        }],
    )
    .await;
    seed_group_routing_provider(
        &state,
        "team-b-provider",
        false,
        vec![team_b_group.id.clone()],
        vec![CreateMonoizeChannelInput {
            id: Some("team-b".to_string()),
            name: "team-b".to_string(),
            provider_type: MonoizeProviderType::Responses,
            base_url: "https://team-b.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
            models: std::collections::HashMap::from([(
                GROUP_ROUTING_MODEL.to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: Multiplier::ONE,
                },
            )]),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            affinity_enabled_override: None,
            affinity_idle_ttl_seconds_override: None,
            affinity_failback_mode_override: None,
            affinity_failback_delay_seconds_override: None,

            proxy_url: None,
            extra_headers: None,
            session_affinity_auto: None,
        }],
    )
    .await;
    seed_model_pricing(&state, GROUP_ROUTING_MODEL).await;

    let req = build_test_routing_request(GROUP_ROUTING_MODEL);

    // R-GRP-1: None marks internal traffic and bypasses group filtering.
    let unrestricted_auth = build_test_auth(None);
    let unrestricted = build_monoize_attempts(&state, &req, &unrestricted_auth)
        .await
        .expect("unrestricted routing succeeds");
    let team_a_auth = build_test_auth(Some(vec![team_a_group.id.clone()]));
    let team_a = build_monoize_attempts(&state, &req, &team_a_auth)
        .await
        .expect("team-a routing succeeds");
    let default_only_auth = build_test_auth(Some(vec![default_group_id]));
    let default_only = build_monoize_attempts(&state, &req, &default_only_auth)
        .await
        .expect("default-group routing succeeds");

    assert_eq!(
        attempt_channel_ids(&unrestricted),
        BTreeSet::from(["public", "team-a", "team-b"])
    );
    assert_eq!(attempt_channel_ids(&team_a), BTreeSet::from(["team-a"]));
    assert_eq!(
        attempt_channel_ids(&default_only),
        BTreeSet::from(["public"])
    );
}

#[tokio::test]
async fn execute_nonstream_typed_keeps_bad_gateway_when_groups_filter_every_channel() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let team_a_group = state
        .user_store
        .create_group(crate::users::CreateGroupInput {
            name: "team-a".to_string(),
            description: String::new(),
            user_selectable: true,
            sort_order: 1,
        })
        .await
        .expect("team-a group created");
    let team_b_group = state
        .user_store
        .create_group(crate::users::CreateGroupInput {
            name: "team-b".to_string(),
            description: String::new(),
            user_selectable: true,
            sort_order: 2,
        })
        .await
        .expect("team-b group created");

    seed_group_routing_provider(
        &state,
        "team-a-provider",
        true,
        vec![team_a_group.id.clone()],
        vec![CreateMonoizeChannelInput {
            id: Some("team-a".to_string()),
            name: "team-a".to_string(),
            provider_type: MonoizeProviderType::Responses,
            base_url: "https://team-a.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
            models: std::collections::HashMap::from([(
                GROUP_ROUTING_MODEL.to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: Multiplier::ONE,
                },
            )]),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            affinity_enabled_override: None,
            affinity_idle_ttl_seconds_override: None,
            affinity_failback_mode_override: None,
            affinity_failback_delay_seconds_override: None,

            proxy_url: None,
            extra_headers: None,
            session_affinity_auto: None,
        }],
    )
    .await;

    let err = execute_nonstream_typed(
        &state,
        &build_test_auth(Some(vec![team_b_group.id.clone()])),
        build_test_urp_request(GROUP_ROUTING_MODEL),
        None,
        DownstreamProtocol::ChatCompletions,
        None,
        None,
        None,
        RequestCaptureContext {
            raw_input: std::sync::Arc::new(json!({})),
            session: None,
        },
    )
    .await
    .expect_err("non-overlapping group restriction should leave no attempts");

    assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    assert_eq!(err.code, "upstream_error");
    assert_eq!(
        err.message,
        format!("No available upstream provider for model: {GROUP_ROUTING_MODEL}")
    );
}

#[test]
fn exhausted_upstream_error_preserves_final_machine_code() {
    let tried = vec![TriedProvider {
        attempt_number: 1,
        provider_id: "provider-a".to_string(),
        channel_id: "channel-a".to_string(),
        provider_name: "Provider A".to_string(),
        channel_name: "Channel A".to_string(),
        error: "upstream status 400 Bad Request: encrypted content could not be verified"
            .to_string(),
        client_error: "encrypted content could not be verified".to_string(),
        upstream_status: Some(StatusCode::BAD_REQUEST.as_u16()),
        upstream_code: Some("thinking_signature_invalid".to_string()),
        upstream_type: Some("invalid_request_error".to_string()),
        upstream_param: None,
        duration_ms: Some(12),
    }];

    let err = build_exhausted_upstream_error("gpt-5.6-sol", &tried);

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert_eq!(err.code, "thinking_signature_invalid");
    assert_eq!(err.message, "encrypted content could not be verified");
    assert_eq!(
        err.internal_message.as_deref(),
        Some("upstream status 400 Bad Request: encrypted content could not be verified")
    );
    assert_eq!(
        err.upstream_code.as_deref(),
        Some("thinking_signature_invalid")
    );
}

const LEAKY_TRANSPORT_ERROR: &str = "error sending request for url (https://api.cloudflare.com/client/v4/accounts/ebb3b05a7371fbcbd62bde8264c86cfe/ai/v1/chat/completions)";

#[test]
fn upstream_error_to_app_replaces_transport_detail_with_generic_message() {
    let err = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Network,
            None,
            LEAKY_TRANSPORT_ERROR.to_string(),
        ),
        true,
    );

    assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    assert_eq!(err.message, "failed to request upstream");
    // SAN-2: the admin-tier internal detail keeps the raw transport text.
    let internal = err.internal_message.as_deref().expect("internal detail");
    assert!(
        internal.starts_with("upstream status 502 Bad Gateway: "),
        "{internal}"
    );
    assert!(internal.contains("api.cloudflare.com"), "{internal}");
    assert!(
        internal.contains("ebb3b05a7371fbcbd62bde8264c86cfe"),
        "{internal}"
    );
}

#[test]
fn upstream_error_to_app_drops_unparsed_error_body_from_client_message() {
    let raw_body = format!("<html>502 Bad Gateway from {LEAKY_TRANSPORT_ERROR}</html>");
    let err = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::BAD_GATEWAY),
            raw_body,
        )
        .with_source(upstream::UpstreamErrorSource::UnparsedBody),
        true,
    );

    assert_eq!(err.message, "upstream status 502 Bad Gateway");
    // SAN-2: the raw unparsed body stays admin-readable in internal detail.
    let internal = err.internal_message.as_deref().expect("internal detail");
    assert!(internal.contains("api.cloudflare.com"), "{internal}");
    assert!(internal.contains("<html>502 Bad Gateway"), "{internal}");
}

#[test]
fn upstream_error_to_app_masks_structured_message_and_keeps_status_prefix() {
    let err = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::UNPROCESSABLE_ENTITY),
            "invalid request".to_string(),
        )
        .with_source(upstream::UpstreamErrorSource::StructuredBody),
        true,
    );
    assert_eq!(
        err.message,
        "upstream status 422 Unprocessable Entity: invalid request"
    );

    let masked = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::UNPROCESSABLE_ENTITY),
            "rejected by https://api.cloudflare.com/client/v4/accounts/abc123/ai".to_string(),
        )
        .with_source(upstream::UpstreamErrorSource::StructuredBody),
        true,
    );
    assert!(
        masked.message.contains("https://***.com/***"),
        "{}",
        masked.message
    );
    assert!(!masked.message.contains("cloudflare"), "{}", masked.message);
}

// SAN-CFG5: with `mask_sensitive_info` disabled, the client message carries
// the raw upstream detail (TRUNC-bounded for transport/unparsed sources).
#[test]
fn upstream_error_to_app_exposes_raw_detail_when_masking_disabled() {
    let transport = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Network,
            None,
            LEAKY_TRANSPORT_ERROR.to_string(),
        ),
        false,
    );
    assert_eq!(
        transport.message,
        format!("upstream status 502 Bad Gateway: {LEAKY_TRANSPORT_ERROR}")
    );

    let raw_body = format!("<html>502 Bad Gateway from {LEAKY_TRANSPORT_ERROR}</html>");
    let unparsed = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::BAD_GATEWAY),
            raw_body.clone(),
        )
        .with_source(upstream::UpstreamErrorSource::UnparsedBody),
        false,
    );
    assert_eq!(
        unparsed.message,
        format!("upstream status 502 Bad Gateway: {raw_body}")
    );

    let empty = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::BAD_GATEWAY),
            "502 Bad Gateway".to_string(),
        )
        .with_source(upstream::UpstreamErrorSource::EmptyBody),
        false,
    );
    assert_eq!(empty.message, "upstream status 502 Bad Gateway");

    let structured = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::UNPROCESSABLE_ENTITY),
            "rejected by https://api.cloudflare.com/client/v4/accounts/abc123/ai".to_string(),
        )
        .with_source(upstream::UpstreamErrorSource::StructuredBody),
        false,
    );
    assert_eq!(
        structured.message,
        "upstream status 422 Unprocessable Entity: rejected by https://api.cloudflare.com/client/v4/accounts/abc123/ai"
    );
}

// SAN-CFG5.2/5.3: transport and unparsed client text is TRUNC-bounded when
// masking is off, so an oversized raw body cannot flood the client message.
#[test]
fn upstream_error_to_app_truncates_raw_client_detail_when_masking_disabled() {
    let oversized = "x".repeat(3000);
    let err = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(StatusCode::BAD_GATEWAY),
            oversized,
        )
        .with_source(upstream::UpstreamErrorSource::UnparsedBody),
        false,
    );
    assert!(err.message.ends_with("... (truncated)"), "{}", err.message);
    assert_eq!(
        err.message.chars().count(),
        "upstream status 502 Bad Gateway: ".chars().count()
            + 2048
            + "... (truncated)".chars().count()
    );
}

#[test]
fn exhausted_error_message_omits_attempt_count_and_infra_detail() {
    let attempt = affinity_test_attempt(
        "provider-leak",
        "channel-leak",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    let app_err = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Network,
            Some(StatusCode::BAD_GATEWAY),
            LEAKY_TRANSPORT_ERROR.to_string(),
        ),
        true,
    );
    let tried = vec![
        TriedProvider::from_app_error(1, &attempt, &app_err, Some(10), true),
        TriedProvider::from_app_error(2, &attempt, &app_err, Some(11), true),
    ];

    let err = build_exhausted_upstream_error("deepseek-v4-flash", &tried);

    assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        err.message,
        "All upstream attempts failed for model: deepseek-v4-flash. Last error: failed to request upstream"
    );
    assert!(!err.message.contains("cloudflare"), "{}", err.message);
    assert!(
        !err.message.contains("2 upstream attempt"),
        "{}",
        err.message
    );

    // SAN-7: the admin-tier internal detail carries the attempt count and the
    // raw last-attempt error.
    let internal = err.internal_message.as_deref().expect("internal detail");
    assert!(
        internal.starts_with("All 2 upstream attempt(s) failed for model: deepseek-v4-flash."),
        "{internal}"
    );
    assert!(internal.contains("api.cloudflare.com"), "{internal}");
    assert!(
        internal.contains("ebb3b05a7371fbcbd62bde8264c86cfe"),
        "{internal}"
    );

    // SAN-5/SAN-10: persisted attempt errors keep the raw detail; the masked
    // client_error never serializes.
    let persisted = serde_json::to_value(&tried).expect("tried providers serialize");
    let serialized = persisted.to_string();
    assert!(!serialized.contains("client_error"), "{serialized}");
    assert!(
        persisted[0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("api.cloudflare.com")),
        "{serialized}"
    );
    assert_eq!(tried[1].client_error, "failed to request upstream");
}

// SAN-CFG5 item 1: with masking disabled `client_error` equals the raw
// AppError message, so the exhausted downstream error carries full detail.
#[test]
fn tried_provider_client_error_keeps_raw_text_when_masking_disabled() {
    let attempt = affinity_test_attempt(
        "provider-raw",
        "channel-raw",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    let app_err = routing::upstream_error_to_app(
        UpstreamCallError::new(
            UpstreamErrorKind::Network,
            Some(StatusCode::BAD_GATEWAY),
            LEAKY_TRANSPORT_ERROR.to_string(),
        ),
        false,
    );
    let tried = vec![TriedProvider::from_app_error(
        1,
        &attempt,
        &app_err,
        Some(10),
        false,
    )];

    assert_eq!(
        tried[0].client_error,
        format!("upstream status 502 Bad Gateway: {LEAKY_TRANSPORT_ERROR}")
    );

    let err = build_exhausted_upstream_error("deepseek-v4-flash", &tried);
    assert!(
        err.message.contains("api.cloudflare.com"),
        "{}",
        err.message
    );
}

#[test]
fn channel_origin_key_groups_same_host_independent_of_path_and_case() {
    assert_eq!(
        routing::channel_origin_key("https://input.codes"),
        routing::channel_origin_key("https://INPUT.CODES/v1/")
    );
    assert_eq!(
        routing::channel_origin_key("https://input.codes").as_deref(),
        Some("https://input.codes:443")
    );
    assert_ne!(
        routing::channel_origin_key("https://input.codes"),
        routing::channel_origin_key("https://codex.ciii.club")
    );
    assert!(routing::channel_origin_key("not a url").is_none());
}

#[test]
fn shared_origin_status_covers_502_503_524_only() {
    assert!(routing::is_shared_origin_status(Some(502)));
    assert!(routing::is_shared_origin_status(Some(503)));
    assert!(routing::is_shared_origin_status(Some(524)));
    assert!(!routing::is_shared_origin_status(Some(429)));
    assert!(!routing::is_shared_origin_status(Some(500)));
    assert!(!routing::is_shared_origin_status(None));
}

#[tokio::test]
async fn shared_origin_blast_marks_peer_channels_and_skips_without_clearing_affinity() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "og86dfgj",
        "input-1",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.channel_id = "input-1".to_string();
    attempt.base_url = "https://input.codes".to_string();
    attempt.origin_key = routing::channel_origin_key(&attempt.base_url);
    attempt.origin_peer_channel_ids = vec!["input-1".to_string(), "input-2".to_string()];
    attempt.passive_failure_count_threshold = 1;
    attempt.affinity_key = Some("v1|api_key:k|model:gpt-affinity|prefix:x".to_string());
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);
    state.channel_affinity.lock().await.insert(
        attempt.affinity_key.clone().expect("key"),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            bound_at: now_ts(),
            last_used_at: now_ts(),
            expires_at: now_ts() + 1800,
        },
    );

    let err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        "upstream status 502 Bad Gateway: error code: 502",
    )
    .with_upstream_error(Some(StatusCode::BAD_GATEWAY), None, None, None);
    let mut tried = Vec::new();
    let mut execution_state = AttemptExecutionState::default();
    routing::record_upstream_attempt_failure(
        &state,
        &attempt,
        1,
        &err,
        Some(routing::RetryableFailureClass::Transient),
        &mut tried,
        &mut execution_state,
    )
    .await;

    assert!(execution_state.should_skip(&attempt));
    let health = state.channel_health.lock().await;
    assert!(!health.get("input-1").expect("input-1 health").healthy);
    assert!(!health.get("input-2").expect("input-2 health").healthy);
    drop(health);
    assert!(
        state
            .channel_affinity
            .lock()
            .await
            .contains_key(attempt.affinity_key.as_ref().expect("key"))
    );
}

#[test]
fn midstream_terminal_failure_class_maps_breaker_relevant_signals() {
    assert_eq!(
        routing::midstream_terminal_failure_class(429, None, None),
        Some(routing::RetryableFailureClass::RateLimited)
    );
    assert_eq!(
        routing::midstream_terminal_failure_class(408, None, None),
        Some(routing::RetryableFailureClass::Transient)
    );
    for status in [500, 502, 503, 529, 599] {
        assert_eq!(
            routing::midstream_terminal_failure_class(status, None, None),
            Some(routing::RetryableFailureClass::Transient),
            "{status}"
        );
    }
    for status in [401, 402, 403, 404, 405, 407, 410, 415, 426, 451] {
        assert_eq!(
            routing::midstream_terminal_failure_class(status, None, None),
            Some(routing::RetryableFailureClass::Persistent),
            "{status}"
        );
    }
    for status in [200, 400, 409, 422] {
        assert_eq!(
            routing::midstream_terminal_failure_class(status, None, None),
            None,
            "{status}"
        );
    }

    assert_eq!(
        routing::midstream_terminal_failure_class(400, None, Some("model_not_found")),
        Some(routing::RetryableFailureClass::Persistent)
    );
    assert_eq!(
        routing::midstream_terminal_failure_class(400, Some("rate_limit_exceeded"), None),
        Some(routing::RetryableFailureClass::RateLimited)
    );
    assert_eq!(
        routing::midstream_terminal_failure_class(400, Some("overloaded_error"), None),
        Some(routing::RetryableFailureClass::Transient)
    );
    assert_eq!(
        routing::midstream_terminal_failure_class(
            403,
            Some("rate_limit_exceeded"),
            None
        ),
        Some(routing::RetryableFailureClass::Persistent)
    );
    assert_eq!(
        routing::midstream_terminal_failure_class(503, None, Some("model_not_found")),
        Some(routing::RetryableFailureClass::Persistent)
    );
}

#[tokio::test]
async fn persistent_failure_trips_breaker_before_sample_threshold() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "persistent-failure-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.passive_failure_count_threshold = 3;
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);

    routing::mark_channel_retryable_failure(
        &state,
        &attempt,
        routing::RetryableFailureClass::Persistent,
    )
    .await;

    let health = state.channel_health.lock().await;
    let entry = health
        .get("persistent-failure-channel")
        .expect("health entry exists");
    assert!(!entry.healthy);
    assert_eq!(entry.passive_failure_timestamps.len(), 1);
}

#[test]
fn upstream_adapter_failure_classification_uses_upstream_prefix() {
    for code in [
        "upstream_idle_timeout",
        "upstream_stream_error",
        "upstream_incomplete_stream",
    ] {
        let err = AppError::new(StatusCode::BAD_GATEWAY, code, "adapter failure");
        assert!(routing::is_upstream_adapter_failure(&err), "{code}");
    }
    for code in ["invalid_upstream_response", "encode_error", "internal_error"] {
        let err = AppError::new(StatusCode::BAD_GATEWAY, code, "internal failure");
        assert!(!routing::is_upstream_adapter_failure(&err), "{code}");
    }
}

#[test]
fn same_channel_attempt_slots_reserve_one_extra_for_affinity_hit() {
    let mut attempt = affinity_test_attempt(
        "provider",
        "slots-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.channel_max_retries = 0;
    assert_eq!(routing::same_channel_attempt_slots(&attempt), 1);

    attempt.channel_max_retries = 2;
    assert_eq!(routing::same_channel_attempt_slots(&attempt), 3);

    attempt.affinity_hit = Some(true);
    assert_eq!(routing::same_channel_attempt_slots(&attempt), 4);

    attempt.affinity_hit = Some(false);
    assert_eq!(routing::same_channel_attempt_slots(&attempt), 3);
}

#[tokio::test]
async fn bound_target_earns_one_extra_transient_attempt_but_not_for_rate_limits() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "extra-slot-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.channel_max_retries = 0;
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);
    let execution_state = AttemptExecutionState::default();

    // RTA-4: without an affinity hit, channel_max_retries = 0 permits no retry.
    assert!(
        !routing::allow_same_channel_retry(
            &state,
            &attempt,
            &execution_state,
            1,
            Some(routing::RetryableFailureClass::Transient),
        )
        .await
    );

    // RTA-4a: the bound target earns exactly one extra Transient attempt.
    attempt.affinity_hit = Some(true);
    assert!(
        routing::allow_same_channel_retry(
            &state,
            &attempt,
            &execution_state,
            1,
            Some(routing::RetryableFailureClass::Transient),
        )
        .await
    );
    assert!(
        !routing::allow_same_channel_retry(
            &state,
            &attempt,
            &execution_state,
            2,
            Some(routing::RetryableFailureClass::Transient),
        )
        .await
    );

    // RTA-4a: a 429 never consumes the extra slot.
    assert!(
        !routing::allow_same_channel_retry(
            &state,
            &attempt,
            &execution_state,
            1,
            Some(routing::RetryableFailureClass::RateLimited),
        )
        .await
    );

    // A non-retryable failure never re-enters the same Channel.
    assert!(!routing::allow_same_channel_retry(&state, &attempt, &execution_state, 1, None).await);
}

#[tokio::test]
async fn same_channel_retry_requires_healthy_channel_and_no_shared_origin_skip() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "gated-retry-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.channel_max_retries = 1;
    attempt.passive_failure_count_threshold = 1;
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);

    let mut execution_state = AttemptExecutionState::default();
    attempt.origin_key = Some("https|origin.example|443".to_string());
    execution_state.mark_shared_origin_skip(&attempt);
    assert!(
        !routing::allow_same_channel_retry(
            &state,
            &attempt,
            &execution_state,
            1,
            Some(routing::RetryableFailureClass::Transient),
        )
        .await
    );

    let execution_state = AttemptExecutionState::default();
    routing::mark_channel_retryable_failure(
        &state,
        &attempt,
        routing::RetryableFailureClass::Transient,
    )
    .await;
    assert!(
        !routing::allow_same_channel_retry(
            &state,
            &attempt,
            &execution_state,
            1,
            Some(routing::RetryableFailureClass::Transient),
        )
        .await
    );
}

#[tokio::test]
async fn sub_threshold_failure_on_bound_target_keeps_affinity_binding() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "sticky-keep-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.passive_failure_count_threshold = 3;
    attempt.affinity_hit = Some(true);
    attempt.affinity_key = Some("v1|api_key:k|model:gpt-affinity|prefix:keep".to_string());
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);
    state.channel_affinity.lock().await.insert(
        attempt.affinity_key.clone().expect("key"),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            bound_at: now_ts(),
            last_used_at: now_ts(),
            expires_at: now_ts() + 1800,
        },
    );

    // 500 is Transient but not a shared-origin status; one sample stays below
    // the threshold of 3, so AFF-9 keeps the binding.
    let err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        "upstream status 500",
    )
    .with_upstream_error(Some(StatusCode::INTERNAL_SERVER_ERROR), None, None, None);
    let mut tried = Vec::new();
    let mut execution_state = AttemptExecutionState::default();
    routing::record_upstream_attempt_failure(
        &state,
        &attempt,
        1,
        &err,
        Some(routing::RetryableFailureClass::Transient),
        &mut tried,
        &mut execution_state,
    )
    .await;

    assert!(
        state
            .channel_affinity
            .lock()
            .await
            .contains_key(attempt.affinity_key.as_ref().expect("key"))
    );
    let health = state.channel_health.lock().await;
    assert!(health.get("sticky-keep-channel").expect("entry").healthy);
}

#[tokio::test]
async fn breaker_tripping_failure_on_bound_target_clears_affinity_binding() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "sticky-clear-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.passive_failure_count_threshold = 1;
    attempt.affinity_hit = Some(true);
    attempt.affinity_key = Some("v1|api_key:k|model:gpt-affinity|prefix:clear".to_string());
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);
    state.channel_affinity.lock().await.insert(
        attempt.affinity_key.clone().expect("key"),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            bound_at: now_ts(),
            last_used_at: now_ts(),
            expires_at: now_ts() + 1800,
        },
    );

    let err = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_error",
        "upstream status 500",
    )
    .with_upstream_error(Some(StatusCode::INTERNAL_SERVER_ERROR), None, None, None);
    let mut tried = Vec::new();
    let mut execution_state = AttemptExecutionState::default();
    routing::record_upstream_attempt_failure(
        &state,
        &attempt,
        1,
        &err,
        Some(routing::RetryableFailureClass::Transient),
        &mut tried,
        &mut execution_state,
    )
    .await;

    assert!(
        !state
            .channel_affinity
            .lock()
            .await
            .contains_key(attempt.affinity_key.as_ref().expect("key"))
    );
    let health = state.channel_health.lock().await;
    assert!(!health.get("sticky-clear-channel").expect("entry").healthy);
}

#[tokio::test]
async fn midstream_terminal_failure_samples_health_and_clears_affinity_only_on_trip() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "midstream-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.passive_failure_count_threshold = 2;
    attempt.affinity_hit = Some(true);
    attempt.affinity_key = Some("v1|api_key:k|model:gpt-affinity|prefix:mid".to_string());
    attempt.origin_key = routing::channel_origin_key("https://midstream.example");
    attempt.origin_peer_channel_ids =
        vec!["midstream-channel".to_string(), "midstream-peer".to_string()];
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);
    state.channel_affinity.lock().await.insert(
        attempt.affinity_key.clone().expect("key"),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: attempt.provider_id.clone(),
            channel_id: attempt.channel_id.clone(),
            bound_at: now_ts(),
            last_used_at: now_ts(),
            expires_at: now_ts() + 1800,
        },
    );

    // STRM-4a: the first sample stays below the threshold of 2 and keeps the binding.
    routing::record_midstream_terminal_failure(
        &state,
        &attempt,
        routing::RetryableFailureClass::Transient,
    )
    .await;
    assert!(
        state
            .channel_affinity
            .lock()
            .await
            .contains_key(attempt.affinity_key.as_ref().expect("key"))
    );

    // The second sample trips the breaker and clears the binding.
    routing::record_midstream_terminal_failure(
        &state,
        &attempt,
        routing::RetryableFailureClass::Transient,
    )
    .await;
    assert!(
        !state
            .channel_affinity
            .lock()
            .await
            .contains_key(attempt.affinity_key.as_ref().expect("key"))
    );
    let health = state.channel_health.lock().await;
    assert!(!health.get("midstream-channel").expect("entry").healthy);
    // STRM-4: mid-stream failures never blast shared-origin peers.
    assert!(!health.contains_key("midstream-peer"));
}

#[tokio::test]
async fn affinity_keeps_unexpired_binding_when_target_is_absent() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,
        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let request = build_test_routing_request("gpt-affinity");
    let auth = affinity_test_auth();
    let (key, _) = affinity_key_for_request(&request, &auth).expect("affinity key");
    let now = now_ts();
    state.channel_affinity.lock().await.insert(
        key.clone(),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: "provider-b".to_string(),
            channel_id: "channel-b1".to_string(),
            bound_at: now,
            last_used_at: now,
            expires_at: now + 1800,
        },
    );

    let attempts = apply_channel_affinity(
        &state,
        &request,
        &auth,
        vec![affinity_test_attempt(
            "provider-a",
            "channel-a1",
            crate::monoize_routing::AffinityFailbackMode::Sticky,
            30,
        )],
    )
    .await
    .expect("affinity applies");

    assert_eq!(attempts[0].affinity_hit, Some(false));
    assert_eq!(
        attempts[0].affinity_target.as_deref(),
        Some("provider-b/channel-b1")
    );
    assert!(state.channel_affinity.lock().await.contains_key(&key));
}

#[test]
fn apply_model_redirects_to_model_uses_first_match_wins() {
    let mut model = "claude-opus-4-6-20250610".to_string();
    apply_model_redirects_to_model(
        &mut model,
        &[
            build_model_redirect_rule(".*opus.*", "gpt-5.4"),
            build_model_redirect_rule("claude-.*", "gpt-5.4-mini"),
        ],
        &[],
    );

    assert_eq!(model, "gpt-5.4");
}

#[test]
fn apply_model_redirects_to_model_uses_global_rule_when_api_key_rules_miss() {
    let mut model = "claude-sonnet-5".to_string();
    apply_model_redirects_to_model(
        &mut model,
        &[build_model_redirect_rule(".*opus.*", "key-target")],
        &[build_model_redirect_rule("claude-.*", "global-target")],
    );

    assert_eq!(model, "global-target");
}

#[test]
fn apply_model_redirects_to_model_does_not_chain_global_after_api_key_match() {
    let mut model = "claude-sonnet-5".to_string();
    apply_model_redirects_to_model(
        &mut model,
        &[build_model_redirect_rule("claude-.*", "intermediate")],
        &[build_model_redirect_rule("intermediate", "global-target")],
    );

    assert_eq!(model, "intermediate");
}

#[test]
fn apply_model_redirects_to_model_leaves_unmatched_model_unchanged() {
    let mut model = "gpt-5-mini".to_string();
    apply_model_redirects_to_model(
        &mut model,
        &[build_model_redirect_rule(".*opus.*", "gpt-5.4")],
        &[build_model_redirect_rule("claude-.*", "gpt-5.4-mini")],
    );

    assert_eq!(model, "gpt-5-mini");
}

fn affinity_test_attempt(
    provider_id: &str,
    channel_id: &str,
    failback_mode: crate::monoize_routing::AffinityFailbackMode,
    failback_delay_seconds: u64,
) -> MonoizeAttempt {
    MonoizeAttempt {
        provider_id: provider_id.to_string(),
        provider_name: provider_id.to_string(),
        provider_type: ProviderType::Responses,
        channel_id: channel_id.to_string(),
        channel_name: channel_id.to_string(),
        base_url: "https://example.com".to_string(),
        api_key: "secret".to_string(),
        logical_model: "gpt-affinity".to_string(),
        upstream_model: "gpt-affinity".to_string(),
        model_multiplier: Multiplier::ONE,
        server_tool_usage_classes: Vec::new(),
        provider_transforms: Vec::new(),
        passive_failure_count_threshold: 3,
        passive_cooldown_seconds: 60,
        passive_window_seconds: 30,
        passive_rate_limit_cooldown_seconds: 15,
        channel_max_retries: 0,
        channel_retry_interval_ms: 0,
        circuit_breaker_enabled: true,
        per_model_circuit_break: false,
        provider_attempt_limit: Some(1),
        request_timeout_ms: 30_000,
        extra_fields_whitelist: None,
        strip_cross_protocol_nested_extra: true,
        billable_pricing_available: true,
        billing_rate_resolution: None,
        affinity_key: None,
        affinity_key_hash: None,
        affinity_hit: None,
        affinity_target: None,
        affinity_enabled: true,
        affinity_idle_ttl_seconds: 1800,
        affinity_failback_mode: failback_mode,
        affinity_failback_delay_seconds: failback_delay_seconds,
        routing_config_revision: 0,
        proxy_url: None,
        extra_headers: None,
        session_affinity_auto: false,
        client_session_id: None,
        derived_session_affinity: None,
        session_affinity_value: None,
        origin_key: None,
        origin_peer_channel_ids: Vec::new(),
    }
}

#[test]
fn session_affinity_resolution_priority_matches_spec() {
    let body = serde_json::json!({
        "prompt_cache_key": "from-cache-key",
        "messages": [{"role": "system", "content": "s"}, {"role": "user", "content": "u"}]
    });
    let base = affinity_test_attempt(
        "p",
        "c",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        0,
    );

    // Disabled channels produce no value (CM-AFF-4 null).
    assert_eq!(resolve_session_affinity_value(&base, &body), None);

    // Auto-enabled without client header derives from the body.
    let mut auto = base.clone();
    auto.session_affinity_auto = true;
    assert_eq!(
        resolve_session_affinity_value(&auto, &body).as_deref(),
        Some("from-cache-key")
    );

    // CM-AFF-1a: client session id wins over derivation.
    let mut client = auto.clone();
    client.client_session_id = Some("client-ses-1".to_string());
    assert_eq!(
        resolve_session_affinity_value(&client, &body).as_deref(),
        Some("client-ses-1")
    );

    // CM-AFF-1: explicit static header wins over client and derivation.
    let mut explicit = client.clone();
    explicit.extra_headers = Some(std::collections::BTreeMap::from([(
        "x-session-affinity".to_string(),
        "static-1".to_string(),
    )]));
    assert_eq!(
        resolve_session_affinity_value(&explicit, &body).as_deref(),
        Some("static-1")
    );

    // The header set applies the resolved value exactly once.
    let headers = attempt_extra_headers(&explicit, &body);
    assert_eq!(
        headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("x-session-affinity"))
            .count(),
        1
    );
    assert!(headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("x-session-affinity") && value == "static-1"
    }));
}

#[test]
fn session_affinity_null_setting_auto_detects_direct_workers_ai_urls() {
    for base_url in [
        "https://api.cloudflare.com/client/v4/accounts/account-id/ai",
        "https://api.cloudflare.com/client/v4/accounts/account-id/ai/",
        "https://api.cloudflare.com/client/v4/accounts/account-id/ai/v1",
    ] {
        assert!(
            routing::effective_session_affinity_auto(base_url, None),
            "{base_url}"
        );
    }

    for base_url in [
        "http://api.cloudflare.com/client/v4/accounts/account-id/ai",
        "https://api.cloudflare.com.example/client/v4/accounts/account-id/ai",
        "https://api.cloudflare.com/client//v4/accounts/account-id/ai",
        "https://api.cloudflare.com/client/v4/accounts/account-id/ai/run/model",
        "https://api.cloudflare.com/client/v4/accounts/account-id/ai//",
        "https://api.cloudflare.com/client/v4/accounts/account-id/ai?debug=1",
        "https://example.com/client/v4/accounts/account-id/ai",
    ] {
        assert!(
            !routing::effective_session_affinity_auto(base_url, None),
            "{base_url}"
        );
    }
}

#[test]
fn session_affinity_explicit_setting_overrides_url_detection() {
    let workers_ai = "https://api.cloudflare.com/client/v4/accounts/account-id/ai";
    assert!(!routing::effective_session_affinity_auto(
        workers_ai,
        Some(false)
    ));
    assert!(routing::effective_session_affinity_auto(
        "https://example.com/v1",
        Some(true)
    ));
}

#[test]
fn client_session_id_header_extraction_prefers_codex_session_id() {
    let mut headers = HeaderMap::new();
    headers.insert("x-session-affinity", "ses-a".parse().unwrap());
    headers.insert("session_id", "codex-b".parse().unwrap());
    assert_eq!(
        extract_client_session_id(&headers).as_deref(),
        Some("codex-b")
    );

    let mut hyphenated = HeaderMap::new();
    hyphenated.insert("session-id", "codex-hyphen".parse().unwrap());
    hyphenated.insert("x-session-affinity", "ses-a".parse().unwrap());
    assert_eq!(
        extract_client_session_id(&hyphenated).as_deref(),
        Some("codex-hyphen")
    );

    let mut x_session_id = HeaderMap::new();
    x_session_id.insert("x-session-id", "sid-1".parse().unwrap());
    assert_eq!(
        extract_client_session_id(&x_session_id).as_deref(),
        Some("sid-1")
    );

    let mut only_affinity = HeaderMap::new();
    only_affinity.insert("x-session-affinity", "ses-a".parse().unwrap());
    assert_eq!(
        extract_client_session_id(&only_affinity).as_deref(),
        Some("ses-a")
    );

    let empty = HeaderMap::new();
    assert_eq!(extract_client_session_id(&empty), None);
}

#[test]
fn session_affinity_sanitizer_strips_controls_and_truncates() {
    let dirty = format!("  ok{}junk", "x".repeat(200));
    let sanitized = sanitize_session_affinity(&dirty);
    assert!(sanitized.starts_with("ok"));
    assert_eq!(sanitized.len(), 128);
    assert!(
        sanitized
            .chars()
            .all(|c| ('\u{20}'..='\u{7e}').contains(&c))
    );

    assert_eq!(sanitize_session_affinity("\u{7}\u{7}"), "");
}

#[test]
fn passive_failure_window_prunes_only_timestamps_before_cutoff() {
    let mut timestamps = std::collections::VecDeque::from([69, 70, 100]);
    routing::prune_passive_failure_timestamps(&mut timestamps, 100, 30);
    assert_eq!(timestamps.into_iter().collect::<Vec<_>>(), vec![70, 100]);
}

#[test]
fn same_channel_retry_classification_covers_all_5xx_and_excludes_client_errors() {
    for status in [
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::INSUFFICIENT_STORAGE,
    ] {
        let err = UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(status),
            "upstream failure".to_string(),
        );
        assert!(routing::is_same_channel_retryable_error(&err), "{status}");
    }

    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::UNPROCESSABLE_ENTITY,
    ] {
        let err = UpstreamCallError::new(
            UpstreamErrorKind::Http,
            Some(status),
            "upstream rejection".to_string(),
        );
        assert!(!routing::is_same_channel_retryable_error(&err), "{status}");
    }

    let network = UpstreamCallError::new(
        UpstreamErrorKind::Network,
        None,
        "connection reset".to_string(),
    );
    assert!(routing::is_same_channel_retryable_error(&network));

    let decode_error = AppError::new(
        StatusCode::BAD_GATEWAY,
        "invalid_upstream_response",
        "invalid response",
    );
    assert!(!routing::is_same_channel_retryable_app_error(&decode_error));

    let embedded_server_error = AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_chat_error",
        "embedded server error",
    )
    .with_upstream_error(
        Some(StatusCode::SERVICE_UNAVAILABLE),
        Some("503".to_string()),
        None,
        None,
    );
    assert!(routing::is_same_channel_retryable_app_error(
        &embedded_server_error
    ));
}

#[tokio::test]
async fn disabled_breaker_success_and_failure_do_not_insert_health_state() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "disabled-breaker-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.circuit_breaker_enabled = false;
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);

    routing::mark_channel_success(&state, &attempt).await;
    routing::mark_channel_retryable_failure(
        &state,
        &attempt,
        routing::RetryableFailureClass::Transient,
    )
    .await;

    assert!(state.channel_health.lock().await.is_empty());
}

#[tokio::test]
async fn passive_failure_queue_stops_at_threshold_and_success_adds_no_sample() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let mut attempt = affinity_test_attempt(
        "provider",
        "bounded-passive-channel",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        30,
    );
    attempt.passive_failure_count_threshold = 2;
    attempt.routing_config_revision = state
        .routing_config_revision
        .load(std::sync::atomic::Ordering::Acquire);

    for _ in 0..3 {
        routing::mark_channel_retryable_failure(
            &state,
            &attempt,
            routing::RetryableFailureClass::Transient,
        )
        .await;
    }
    {
        let health = state.channel_health.lock().await;
        let entry = health
            .get("bounded-passive-channel")
            .expect("health entry exists");
        assert!(!entry.healthy);
        assert_eq!(entry.passive_failure_timestamps.len(), 2);
    }

    routing::mark_channel_success(&state, &attempt).await;
    let health = state.channel_health.lock().await;
    let entry = health
        .get("bounded-passive-channel")
        .expect("health entry exists");
    assert!(entry.healthy);
    assert_eq!(entry.passive_failure_timestamps.len(), 2);
}

fn affinity_test_auth() -> AuthResult {
    let mut auth = build_test_auth(None);
    auth.user_id = Some("affinity-user".to_string());
    auth
}

#[test]
fn affinity_capacity_refreshes_existing_key_and_rejects_new_key() {
    let binding =
        |channel_id: &str, last_used_at: i64| crate::monoize_routing::ChannelAffinityBinding {
            provider_id: "provider".to_string(),
            channel_id: channel_id.to_string(),
            bound_at: 1,
            last_used_at,
            expires_at: last_used_at + 100,
        };
    let mut cache = HashMap::from([("existing".to_string(), binding("old", 1))]);
    routing::insert_channel_affinity_with_limit(
        &mut cache,
        "new".to_string(),
        binding("new", 2),
        1,
    );
    assert_eq!(cache.len(), 1);
    assert!(!cache.contains_key("new"));

    routing::insert_channel_affinity_with_limit(
        &mut cache,
        "existing".to_string(),
        binding("refreshed", 3),
        1,
    );
    assert_eq!(cache["existing"].channel_id, "refreshed");
    assert_eq!(cache["existing"].last_used_at, 3);

    assert_eq!(
        crate::monoize_routing::cleanup_channel_affinity(&mut cache, 103),
        1
    );
    routing::insert_channel_affinity_with_limit(
        &mut cache,
        "new".to_string(),
        binding("new", 104),
        1,
    );
    assert!(cache.contains_key("new"));
}

#[tokio::test]
async fn affinity_lookup_removes_only_the_requested_expired_binding() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let request = build_test_routing_request("gpt-affinity");
    let auth = affinity_test_auth();
    let (key, _) = affinity_key_for_request(&request, &auth).expect("affinity key");
    let now = now_ts();
    let live_key = "unrelated-live".to_string();
    let binding =
        |channel_id: &str, expires_at: i64| crate::monoize_routing::ChannelAffinityBinding {
            provider_id: "provider-b".to_string(),
            channel_id: channel_id.to_string(),
            bound_at: now - 60,
            last_used_at: now - 30,
            expires_at,
        };
    state.channel_affinity.lock().await.extend([
        (key.clone(), binding("channel-b1", now)),
        (live_key.clone(), binding("channel-live", now + 60)),
    ]);

    let attempts = apply_channel_affinity(
        &state,
        &request,
        &auth,
        vec![affinity_test_attempt(
            "provider-b",
            "channel-b1",
            crate::monoize_routing::AffinityFailbackMode::Sticky,
            30,
        )],
    )
    .await
    .expect("affinity applies");

    assert_eq!(attempts[0].affinity_hit, Some(false));
    let cache = state.channel_affinity.lock().await;
    assert!(!cache.contains_key(&key));
    assert!(cache.contains_key(&live_key));
}

#[tokio::test]
async fn affinity_failback_mode_controls_recovered_provider_order() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let request = build_test_routing_request("gpt-affinity");
    let auth = affinity_test_auth();
    let (key, _) = affinity_key_for_request(&request, &auth).expect("affinity key");
    let now = now_ts();
    state.channel_affinity.lock().await.insert(
        key.clone(),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: "provider-b".to_string(),
            channel_id: "channel-b1".to_string(),
            bound_at: now - 60,
            last_used_at: now,
            expires_at: now + 1800,
        },
    );

    let sticky = apply_channel_affinity(
        &state,
        &request,
        &auth,
        vec![
            affinity_test_attempt(
                "provider-a",
                "channel-a1",
                crate::monoize_routing::AffinityFailbackMode::Sticky,
                30,
            ),
            affinity_test_attempt(
                "provider-b",
                "channel-b1",
                crate::monoize_routing::AffinityFailbackMode::Sticky,
                30,
            ),
        ],
    )
    .await
    .expect("sticky affinity applies");
    assert_eq!(sticky[0].channel_id, "channel-b1");
    assert_eq!(sticky[0].affinity_hit, Some(true));

    let failback = apply_channel_affinity(
        &state,
        &request,
        &auth,
        vec![
            affinity_test_attempt(
                "provider-a",
                "channel-a1",
                crate::monoize_routing::AffinityFailbackMode::Sticky,
                30,
            ),
            affinity_test_attempt(
                "provider-b",
                "channel-b1",
                crate::monoize_routing::AffinityFailbackMode::PreferHigherPriority,
                30,
            ),
        ],
    )
    .await
    .expect("failback affinity applies");
    assert_eq!(failback[0].channel_id, "channel-a1");
    assert_eq!(failback[0].affinity_hit, Some(false));
    assert_eq!(
        failback[0].affinity_target.as_deref(),
        Some("provider-b/channel-b1")
    );
}

#[tokio::test]
async fn affinity_disabled_override_removes_existing_binding() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let request = build_test_routing_request("gpt-affinity");
    let auth = affinity_test_auth();
    let (key, _) = affinity_key_for_request(&request, &auth).expect("affinity key");
    let now = now_ts();
    state.channel_affinity.lock().await.insert(
        key.clone(),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: "provider-b".to_string(),
            channel_id: "channel-b1".to_string(),
            bound_at: now,
            last_used_at: now,
            expires_at: now + 1800,
        },
    );
    let mut disabled = affinity_test_attempt(
        "provider-b",
        "channel-b1",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        300,
    );
    disabled.affinity_enabled = false;

    let attempts = apply_channel_affinity(&state, &request, &auth, vec![disabled])
        .await
        .expect("disabled affinity applies");

    assert_eq!(attempts[0].affinity_hit, Some(false));
    assert!(!state.channel_affinity.lock().await.contains_key(&key));
}

#[tokio::test]
async fn response_id_affinity_inherits_source_binding_age_on_hit() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
        request_log_spool_dir: None,

        node: crate::node_config::NodeSettings::primary_default(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");
    let request = build_test_routing_request("gpt-affinity");
    let auth = affinity_test_auth();
    let (key, _) = affinity_key_for_request(&request, &auth).expect("affinity key");
    let now = now_ts();
    let original_bound_at = now - 120;
    state.channel_affinity.lock().await.insert(
        key.clone(),
        crate::monoize_routing::ChannelAffinityBinding {
            provider_id: "provider-b".to_string(),
            channel_id: "channel-b1".to_string(),
            bound_at: original_bound_at,
            last_used_at: now,
            expires_at: now + 1800,
        },
    );
    let mut attempt = affinity_test_attempt(
        "provider-b",
        "channel-b1",
        crate::monoize_routing::AffinityFailbackMode::PreferHigherPriority,
        300,
    );
    attempt.affinity_key = Some(key);
    attempt.affinity_hit = Some(true);

    refresh_channel_affinity(&state, &attempt).await;
    refresh_response_id_affinity(&state, &auth, "gpt-affinity", "resp-next", &attempt).await;

    let response_key = response_id_affinity_key("gpt-affinity", "resp-next", &auth)
        .expect("response affinity key");
    let response_binding = state
        .channel_affinity
        .lock()
        .await
        .get(&response_key)
        .cloned()
        .expect("response binding");
    assert_eq!(response_binding.bound_at, original_bound_at);
    assert_eq!(
        response_binding.expires_at - response_binding.last_used_at,
        attempt.affinity_idle_ttl_seconds as i64
    );
}

#[test]
fn provider_attempt_budget_survives_affinity_interleaving() {
    let provider_a = affinity_test_attempt(
        "provider-a",
        "channel-a1",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        300,
    );
    let provider_b = affinity_test_attempt(
        "provider-b",
        "channel-b1",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        300,
    );
    let mut execution = AttemptExecutionState::default();

    execution.record_upstream_attempt(&provider_b);
    execution.record_upstream_attempt(&provider_a);

    assert!(!execution.provider_budget_remaining(&provider_b));
    assert!(!execution.provider_budget_remaining(&provider_a));
}

#[test]
fn session_affinity_is_stable_within_conversation_and_differs_across_sessions() {
    let base = serde_json::json!({
        "model": "cf-model",
        "messages": [
            { "role": "system", "content": "You are a helpful assistant." },
            { "role": "user", "content": "solve task A" }
        ],
        "tools": [{ "type": "function", "function": { "name": "calc" } }]
    });
    let grown = {
        let mut body = base.clone();
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "role": "assistant", "content": "working" }));
        body
    };
    let first = routing::derive_session_affinity(&base).unwrap();
    let second = routing::derive_session_affinity(&grown).unwrap();
    assert_eq!(
        first, second,
        "appending messages must keep affinity stable"
    );

    let mut tools_changed = base.clone();
    tools_changed["tools"] = serde_json::json!([
        { "type": "function", "function": { "name": "calc" } },
        { "type": "function", "function": { "name": "search" } }
    ]);
    assert_eq!(
        routing::derive_session_affinity(&tools_changed).unwrap(),
        first,
        "tool-definition changes must not split session affinity"
    );

    let other_session = serde_json::json!({
        "model": "cf-model",
        "messages": [
            { "role": "system", "content": "You are a helpful assistant." },
            { "role": "user", "content": "solve task B" }
        ]
    });
    let other = routing::derive_session_affinity(&other_session).unwrap();
    assert_ne!(
        first, other,
        "distinct sessions must derive distinct affinities"
    );
}

#[test]
fn session_affinity_body_session_id_wins_over_derived_digest() {
    let req = urp::decode::openai_chat::decode_request(&serde_json::json!({
        "model": "cf-model",
        "messages": [
            { "role": "system", "content": "shared system prompt" },
            { "role": "user", "content": "turn one" }
        ],
        "tools": [{ "type": "function", "function": { "name": "calc" } }],
        "session_id": "019ffeb5-e6ed-7180-89b6-df6e938625a6"
    }))
    .expect("decode request");
    let mut attempt = affinity_test_attempt(
        "p",
        "c",
        crate::monoize_routing::AffinityFailbackMode::Sticky,
        0,
    );
    attempt.session_affinity_auto = true;
    attach_client_session_id(std::slice::from_mut(&mut attempt), None, Some(&req));
    assert_eq!(
        attempt.client_session_id.as_deref(),
        Some("019ffeb5-e6ed-7180-89b6-df6e938625a6")
    );

    let mut later = req.clone();
    later.input.push(urp::Node::Text {
        id: None,
        role: urp::OrdinaryRole::User,
        content: "turn two".to_string(),
        phase: None,
        extra_body: HashMap::new(),
    });
    later.tools = Some(Vec::new());
    let mut later_attempt = attempt.clone();
    attach_client_session_id(std::slice::from_mut(&mut later_attempt), None, Some(&later));
    assert_eq!(later_attempt.client_session_id, attempt.client_session_id);
    assert_eq!(
        resolve_session_affinity_value(&attempt, &serde_json::json!({})).as_deref(),
        Some("019ffeb5-e6ed-7180-89b6-df6e938625a6")
    );
}

#[test]
fn session_affinity_urp_digest_ignores_tools_and_appended_nodes() {
    let first = urp::decode::openai_chat::decode_request(&serde_json::json!({
        "model": "cf-model",
        "messages": [
            { "role": "system", "content": "shared system prompt" },
            { "role": "user", "content": "session A question" }
        ],
        "tools": [{ "type": "function", "function": { "name": "calc" } }]
    }))
    .expect("decode first");
    let second = urp::decode::openai_chat::decode_request(&serde_json::json!({
        "model": "cf-model",
        "messages": [
            { "role": "system", "content": "shared system prompt" },
            { "role": "user", "content": "session A question" },
            { "role": "assistant", "content": "working" },
            { "role": "user", "content": "continue" }
        ],
        "tools": [
            { "type": "function", "function": { "name": "calc" } },
            { "type": "function", "function": { "name": "search" } }
        ]
    }))
    .expect("decode second");
    assert_eq!(
        routing::derive_session_affinity_from_urp(&first),
        routing::derive_session_affinity_from_urp(&second)
    );

    let other = urp::decode::openai_chat::decode_request(&serde_json::json!({
        "model": "cf-model",
        "messages": [
            { "role": "system", "content": "shared system prompt" },
            { "role": "user", "content": "session B question" }
        ]
    }))
    .expect("decode other");
    assert_ne!(
        routing::derive_session_affinity_from_urp(&first),
        routing::derive_session_affinity_from_urp(&other)
    );
}

#[test]
fn session_affinity_prefers_prompt_cache_key_and_sanitizes() {
    let keyed = serde_json::json!({ "prompt_cache_key": "  sess-42  " });
    assert_eq!(routing::derive_session_affinity(&keyed).unwrap(), "sess-42");
}

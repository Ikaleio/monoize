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
use crate::users::{ModelRedirectRule, RequestCaptureMode, UserRole};
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

fn build_model_redirect_rule(pattern: &str, replace: &str) -> ModelRedirectRule {
    ModelRedirectRule {
        pattern: pattern.to_string(),
        replace: replace.to_string(),
    }
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
    groups: Vec<String>,
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
            models: std::collections::HashMap::from([(
                GROUP_ROUTING_MODEL.to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: 1.0,
                },
            )]),
            groups,
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
                models_dev_provider: Some("openai".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("1000".to_string()),
                output_cost_per_token_nano: Some("1000".to_string()),
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.234_567_891);

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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0);

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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0);

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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0);

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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0);

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

    let components =
        calculate_rate_matrix_charge_components(&usage, None, &resolution, 1.0, &Vec::new())
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

    let components =
        calculate_rate_matrix_charge_components(&usage, None, &resolution, 1.0, &Vec::new())
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

    let err = calculate_rate_matrix_charge_components(&usage, None, &resolution, 1.0, &Vec::new())
        .expect_err("aggregate cache write must not be guessed");

    assert!(err.contains("requires 5m/1h split"));
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

    let components =
        calculate_rate_matrix_charge_components(&usage, None, &resolution, 1.0, &Vec::new())
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

    let components =
        calculate_rate_matrix_charge_components(&usage, None, &resolution, 1.0, &Vec::new())
            .expect("charge succeeds");

    assert_eq!(components.base_charge, 170);
    assert_eq!(
        components.token_line_items[1]["usage_class"].as_str(),
        Some("input_cached")
    );
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
        &call_resolution,
        1.0,
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
        &duration_resolution,
        1.0,
        &["code_interpreter_duration".to_string()],
    )
    .expect_err("duration meter must require authoritative usage");

    assert!(err.contains("authoritative usage required"));
}

#[test]
fn scale_charge_quantizes_multiplier_to_nano_precision() {
    let base = 1_000_000_000i128;
    let charged = scale_charge_with_multiplier(base, 1.000_000_000_9);
    assert_eq!(charged, Some(1_000_000_000));
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
            groups: Vec::new(),
            enabled: true,
            priority: Some(0),
            models: std::collections::HashMap::from([(
                "gpt-5-mini".to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: 1.0,
                },
            )]),
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
                supported_models: vec!["gpt-5-mini".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
            }],
        })
        .await
        .expect("provider created");

    let mut req = build_test_urp_request("gpt-5-mini-thinking");
    resolve_model_suffix(&state, &mut req).await;
    let original_req = req.clone();

    assert_eq!(original_req.model, "gpt-5-mini");
    assert_eq!(
        original_req
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.effort.as_deref()),
        Some("high")
    );
}

#[test]
fn resolve_upstream_model_prefers_non_empty_redirect() {
    let entry = MonoizeModelEntry {
        redirect: Some("  gpt-5-target  ".to_string()),
        multiplier: 1.0,
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
        multiplier: 1.0,
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
            groups: Vec::new(),
            enabled: true,
            priority: Some(0),
            models: std::collections::HashMap::from([(
                "gpt-unpriced".to_string(),
                MonoizeModelEntry {
                    redirect: Some("gpt-unpriced-upstream".to_string()),
                    multiplier: 1.0,
                },
            )]),
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
                supported_models: vec!["gpt-unpriced".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
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
            groups: Vec::new(),
            enabled: true,
            priority: Some(0),
            models: std::collections::HashMap::from([(
                "gpt-unpriced".to_string(),
                MonoizeModelEntry {
                    redirect: Some("gpt-unpriced-upstream".to_string()),
                    multiplier: 1.0,
                },
            )]),
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
                supported_models: vec!["gpt-unpriced".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
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
            groups: Vec::new(),
            enabled: true,
            priority: Some(0),
            models: std::collections::HashMap::from([(
                "gpt-priced".to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: 1.0,
                },
            )]),
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
                supported_models: vec!["gpt-priced".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
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
            groups: Vec::new(),
            enabled: true,
            priority: Some(0),
            models: std::collections::HashMap::from([(
                "gpt-fallback-src".to_string(),
                MonoizeModelEntry {
                    redirect: Some("gpt-fallback-dest".to_string()),
                    multiplier: 1.0,
                },
            )]),
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
                supported_models: vec!["gpt-fallback-src".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
            }],
        })
        .await
        .expect("provider created");

    state
        .model_registry_store
        .upsert_model_metadata(
            "gpt-fallback-src",
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some("openai".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("1000".to_string()),
                output_cost_per_token_nano: Some("1000".to_string()),
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
                models_dev_provider: Some("openai".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("500".to_string()),
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
            groups: Vec::new(),
            enabled: true,
            priority: Some(0),
            models: std::collections::HashMap::from([(
                "claude-sonnet-4.6".to_string(),
                MonoizeModelEntry {
                    redirect: None,
                    multiplier: 1.0,
                },
            )]),
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
                supported_models: vec!["claude-sonnet-4.6".to_string()],
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
            }],
        })
        .await
        .expect("provider created");

    state
        .model_registry_store
        .upsert_model_metadata(
            "claude-sonnet-4.6",
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some("zenmux".to_string()),
                mode: Some("chat".to_string()),
                input_cost_per_token_nano: Some("3000".to_string()),
                output_cost_per_token_nano: Some("15000".to_string()),
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
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

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
            supported_models: vec![GROUP_ROUTING_MODEL.to_string()],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
        }],
    )
    .await;
    seed_group_routing_provider(
        &state,
        "team-a-provider",
        false,
        vec!["team-a".to_string()],
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
            supported_models: vec![GROUP_ROUTING_MODEL.to_string()],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
        }],
    )
    .await;
    seed_group_routing_provider(
        &state,
        "team-b-provider",
        false,
        vec!["team-b".to_string()],
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
            supported_models: vec![GROUP_ROUTING_MODEL.to_string()],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
        }],
    )
    .await;
    seed_model_pricing(&state, GROUP_ROUTING_MODEL).await;

    let req = build_test_routing_request(GROUP_ROUTING_MODEL);

    let unrestricted_auth = build_test_auth(None);
    let unrestricted = build_monoize_attempts(&state, &req, &unrestricted_auth)
        .await
        .expect("unrestricted routing succeeds");
    let team_a_auth = build_test_auth(Some(vec!["team-a".to_string()]));
    let team_a = build_monoize_attempts(&state, &req, &team_a_auth)
        .await
        .expect("team-a routing succeeds");
    let public_only_auth = build_test_auth(Some(Vec::new()));
    let public_only = build_monoize_attempts(&state, &req, &public_only_auth)
        .await
        .expect("public-only routing succeeds");

    assert_eq!(
        attempt_channel_ids(&unrestricted),
        BTreeSet::from(["public", "team-a", "team-b"])
    );
    assert_eq!(attempt_channel_ids(&team_a), BTreeSet::from(["team-a"]));
    assert_eq!(
        attempt_channel_ids(&public_only),
        BTreeSet::from(["public"])
    );
}

#[tokio::test]
async fn execute_nonstream_typed_keeps_bad_gateway_when_groups_filter_every_channel() {
    let runtime = RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        database_dsn: "sqlite::memory:".to_string(),
    };
    let state = load_state_with_runtime(runtime).await.expect("state loads");

    seed_group_routing_provider(
        &state,
        "team-a-provider",
        true,
        vec!["team-a".to_string()],
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
            supported_models: vec![GROUP_ROUTING_MODEL.to_string()],
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
        }],
    )
    .await;

    let err = execute_nonstream_typed(
        &state,
        &build_test_auth(Some(Vec::new())),
        build_test_urp_request(GROUP_ROUTING_MODEL),
        None,
        DownstreamProtocol::ChatCompletions,
        None,
        None,
        RequestCaptureContext {
            raw_input: json!({}),
            session: None,
        },
    )
    .await
    .expect_err("public-only restriction should leave no attempts");

    assert_eq!(err.status, StatusCode::BAD_GATEWAY);
    assert_eq!(err.code, "upstream_error");
    assert_eq!(
        err.message,
        format!("No available upstream provider for model: {GROUP_ROUTING_MODEL}")
    );
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
    );

    assert_eq!(model, "gpt-5.4");
}

#[test]
fn apply_model_redirects_to_model_leaves_unmatched_model_unchanged() {
    let mut model = "gpt-5-mini".to_string();
    apply_model_redirects_to_model(
        &mut model,
        &[build_model_redirect_rule(".*opus.*", "gpt-5.4")],
    );

    assert_eq!(model, "gpt-5-mini");
}

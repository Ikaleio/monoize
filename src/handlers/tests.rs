use super::*;
use crate::app::{RuntimeConfig, load_state_with_runtime};
use crate::auth::AuthResult;
use crate::config::ProviderType;
use crate::model_registry_store::ModelPricing;
use crate::monoize_routing::{
    CreateMonoizeChannelInput, CreateMonoizeProviderInput, MonoizeModelEntry, MonoizeProviderType,
};
use crate::settings::normalize_pricing_model_key;
use crate::urp;
use axum::http::StatusCode;
use std::collections::{BTreeSet, HashMap};

const GROUP_ROUTING_MODEL: &str = "gpt-group-routing";

fn build_test_auth(effective_groups: Option<Vec<String>>) -> AuthResult {
    AuthResult {
        tenant_id: "tenant-1".to_string(),
        user_id: None,
        username: None,
        api_key_id: None,
        max_multiplier: None,
        transforms: Vec::new(),
        effective_groups,
        model_limits_enabled: false,
        model_limits: Vec::new(),
        ip_whitelist: Vec::new(),
        quota_remaining: None,
        quota_unlimited: true,
    }
}

fn build_test_urp_request(model: &str) -> urp::UrpRequest {
    urp::UrpRequest {
        model: model.to_string(),
        inputs: vec![urp::Item::Message {
            role: urp::Role::User,
            parts: vec![urp::Part::Text {
                content: "hello".to_string(),
                extra_body: HashMap::new(),
            }],
            extra_body: HashMap::new(),
        }],
        stream: Some(false),
        temperature: None,
        top_p: None,
        max_output_tokens: None,
        reasoning: None,
        tools: None,
        tool_choice: None,
        response_format: None,
        user: None,
        extra_body: HashMap::new(),
    }
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
            provider_type: MonoizeProviderType::Responses,
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
                models_dev_provider: Some("test".to_string()),
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.234_567_891, ProviderType::Responses);

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
            cache_creation_tokens: 0,
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0, ProviderType::Responses);

    assert_eq!(charged, Some(236_000));
}

#[test]
fn calculate_charge_nano_messages_treats_cache_creation_as_disjoint_bucket() {
    let usage = urp::Usage {
        input_tokens: 100,
        output_tokens: 20,
        input_details: Some(urp::InputDetails {
            standard_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 40,
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0, ProviderType::Messages);

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
            cache_creation_tokens: 40,
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0, ProviderType::Responses);

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
            cache_creation_tokens: 20,
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0, ProviderType::Responses);

    assert_eq!(charged, Some(78_000));
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
            provider_type: MonoizeProviderType::Responses,
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
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
        })
        .await
        .expect("provider created");

    let req = UrpRequest {
        model: "gpt-unpriced".to_string(),
        max_multiplier: None,
    };
    let err = build_monoize_attempts(&state, &req, None)
        .await
        .expect_err("must reject unpriced model");

    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert_eq!(err.code, "model_pricing_required");
    assert!(err.message.contains("gpt-unpriced-upstream"));
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
            provider_type: MonoizeProviderType::Responses,
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
                base_url: "https://example.com".to_string(),
                api_key: Some("secret".to_string()),
                enabled: true,
                weight: 1,
                passive_failure_count_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
        })
        .await
        .expect("provider created");

    state
        .model_registry_store
        .upsert_model_metadata(
            "gpt-fallback-src",
            crate::model_registry_store::UpsertModelMetadataInput {
                models_dev_provider: Some("test".to_string()),
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

    let req = UrpRequest {
        model: "gpt-fallback-src".to_string(),
        max_multiplier: None,
    };
    let attempts = build_monoize_attempts(&state, &req, None)
        .await
        .expect("fallback-priced model should be allowed");

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].upstream_model, "gpt-fallback-dest");
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
            base_url: "https://public.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
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
            base_url: "https://team-a.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
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
            base_url: "https://team-b.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
        }],
    )
    .await;
    seed_model_pricing(&state, GROUP_ROUTING_MODEL).await;

    let req = UrpRequest {
        model: GROUP_ROUTING_MODEL.to_string(),
        max_multiplier: None,
    };

    let unrestricted = build_monoize_attempts(&state, &req, None)
        .await
        .expect("unrestricted routing succeeds");
    let team_a = build_monoize_attempts(&state, &req, Some(vec!["team-a".to_string()]))
        .await
        .expect("team-a routing succeeds");
    let public_only = build_monoize_attempts(&state, &req, Some(Vec::new()))
        .await
        .expect("public-only routing succeeds");

    assert_eq!(
        attempt_channel_ids(&unrestricted),
        BTreeSet::from(["public", "team-a", "team-b"])
    );
    assert_eq!(
        attempt_channel_ids(&team_a),
        BTreeSet::from(["public", "team-a"])
    );
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
            base_url: "https://team-a.example.com".to_string(),
            api_key: Some("secret".to_string()),
            enabled: true,
            weight: 1,
            passive_failure_count_threshold_override: None,
            passive_cooldown_seconds_override: None,
            passive_window_seconds_override: None,
            passive_rate_limit_cooldown_seconds_override: None,
        }],
    )
    .await;

    let err = execute_nonstream_typed(
        &state,
        &build_test_auth(Some(Vec::new())),
        build_test_urp_request(GROUP_ROUTING_MODEL),
        None,
        None,
        None,
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

use super::*;
use crate::app::{RuntimeConfig, load_state_with_runtime};
use crate::config::ProviderType;
use crate::model_registry_store::ModelPricing;
use crate::monoize_routing::{
    CreateMonoizeChannelInput, CreateMonoizeProviderInput, MonoizeModelEntry, MonoizeProviderType,
};
use crate::urp;
use axum::http::StatusCode;
use std::collections::HashMap;

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
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
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
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
                passive_rate_limit_cooldown_seconds_override: None,
            }],
        })
        .await
        .expect("provider created");

    let req = UrpRequest {
        model: "gpt-unpriced".to_string(),
        max_multiplier: None,
    };
    let err = build_monoize_attempts(&state, &req)
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
            transforms: Vec::new(),
            api_type_overrides: Vec::new(),
            active_probe_enabled_override: None,
            active_probe_interval_seconds_override: None,
            active_probe_success_threshold_override: None,
            active_probe_model_override: None,
            request_timeout_ms_override: None,
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
                passive_failure_threshold_override: None,
                passive_cooldown_seconds_override: None,
                passive_window_seconds_override: None,
                passive_min_samples_override: None,
                passive_failure_rate_threshold_override: None,
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
    let attempts = build_monoize_attempts(&state, &req)
        .await
        .expect("fallback-priced model should be allowed");

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].upstream_model, "gpt-fallback-dest");
}

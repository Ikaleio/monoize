use super::*;
use crate::model_registry_store::ModelPricing;
use crate::monoize_routing::MonoizeModelEntry;
use crate::urp;
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

    let charged = calculate_charge_nano(&usage, &pricing, 1.0);

    assert_eq!(charged, Some(236_000));
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

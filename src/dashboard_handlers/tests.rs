use super::providers::{build_models_list_url, provider_pricing_model};
use crate::monoize_routing::MonoizeModelEntry;

#[test]
fn build_models_list_url_adds_v1_when_missing() {
    assert_eq!(
        build_models_list_url("https://openrouter.ai/api"),
        "https://openrouter.ai/api/v1/models"
    );
}

#[test]
fn build_models_list_url_avoids_duplicate_v1_suffix() {
    assert_eq!(
        build_models_list_url("https://openrouter.ai/api/v1"),
        "https://openrouter.ai/api/v1/models"
    );
    assert_eq!(
        build_models_list_url("https://openrouter.ai/api/v1/"),
        "https://openrouter.ai/api/v1/models"
    );
}

#[test]
fn provider_pricing_model_uses_redirect_when_present() {
    let entry = MonoizeModelEntry {
        redirect: Some("  gpt-5-target  ".to_string()),
        multiplier: 1.0,
    };
    assert_eq!(
        provider_pricing_model("gpt-5-logical", &entry),
        "gpt-5-target"
    );
}

#[test]
fn provider_pricing_model_falls_back_to_logical_when_redirect_blank() {
    let entry = MonoizeModelEntry {
        redirect: Some("   ".to_string()),
        multiplier: 1.0,
    };
    assert_eq!(
        provider_pricing_model("gpt-5-logical", &entry),
        "gpt-5-logical"
    );
}

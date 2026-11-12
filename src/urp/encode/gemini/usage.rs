//! Gemini `usageMetadata` encoding (GEM-7, PG6, PG6a).

use crate::urp::encode::{usage_input_details, usage_output_details};
use crate::urp::{ModalityBreakdown, Usage, is_internal_key};
use serde_json::{Map, Value, json};

/// The documented UsageMetadata counters of `usage`. Prompt and candidate counts exclude the
/// disjoint tool-prompt and thought counters; Monoize extension counters appear only when
/// non-zero.
pub(super) fn encode_usage_metadata(usage: &Usage) -> Value {
    let usage = usage.accounting();
    let input = usage_input_details(&usage);
    let output = usage_output_details(&usage);
    let mut obj = Map::new();
    for (key, details) in [
        ("promptTokensDetails", input.modality_breakdown.as_ref()),
        (
            "cacheTokensDetails",
            input.cache_read_modality_breakdown.as_ref(),
        ),
        (
            "toolUsePromptTokensDetails",
            input.tool_prompt_modality_breakdown.as_ref(),
        ),
        (
            "candidatesTokensDetails",
            output.modality_breakdown.as_ref(),
        ),
    ] {
        if let Some(details) = details {
            obj.insert(key.into(), encode_modality(details));
        }
    }
    for (key, count) in [
        (
            "promptTokenCount",
            usage.input_tokens.saturating_sub(input.tool_prompt_tokens),
        ),
        (
            "candidatesTokenCount",
            usage.output_tokens.saturating_sub(output.reasoning_tokens),
        ),
        ("totalTokenCount", usage.total_tokens()),
        ("thoughtsTokenCount", output.reasoning_tokens),
        ("cachedContentTokenCount", input.cache_read_tokens),
        ("toolUsePromptTokenCount", input.tool_prompt_tokens),
    ] {
        obj.insert(key.into(), Value::from(count));
    }
    for (key, count) in [
        ("cacheCreationTokenCount", input.cache_creation_tokens),
        (
            "acceptedPredictionOutputTokenCount",
            output.accepted_prediction_tokens,
        ),
        (
            "rejectedPredictionOutputTokenCount",
            output.rejected_prediction_tokens,
        ),
    ] {
        if count > 0 {
            obj.insert(key.into(), Value::from(count));
        }
    }
    for (key, value) in &usage.extra_body {
        // Typed modality breakdowns own the detail members, including their deletion.
        if !is_internal_key(key)
            && !matches!(
                key.as_str(),
                "promptTokensDetails"
                    | "cacheTokensDetails"
                    | "candidatesTokensDetails"
                    | "toolUsePromptTokensDetails"
            )
        {
            obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    Value::Object(obj)
}

fn encode_modality(details: &ModalityBreakdown) -> Value {
    Value::Array(
        [
            ("TEXT", details.text_tokens),
            ("IMAGE", details.image_tokens),
            ("AUDIO", details.audio_tokens),
            ("VIDEO", details.video_tokens),
            ("DOCUMENT", details.document_tokens),
        ]
        .into_iter()
        .filter_map(|(modality, count)| {
            count.map(|count| json!({"modality": modality, "tokenCount": count}))
        })
        .collect(),
    )
}

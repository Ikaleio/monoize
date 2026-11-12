use crate::urp::decode::{deserialize_u64ish_default, retain_wire_extra_fields, value_to_u64};
use crate::urp::{InputDetails, OutputDetails, Usage, is_internal_key};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "prompt_tokens"
    )]
    input_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "completion_tokens"
    )]
    output_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens",
        alias = "cached_tokens"
    )]
    cache_read_input_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_creation_tokens",
        alias = "cache_write_tokens"
    )]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_creation: AnthropicCacheCreationUsage,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "tool_prompt_input_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "reasoning_output_tokens"
    )]
    reasoning_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "accepted_prediction_output_tokens"
    )]
    accepted_prediction_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "rejected_prediction_output_tokens"
    )]
    rejected_prediction_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
struct AnthropicCacheCreationUsage {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    ephemeral_5m_input_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    ephemeral_1h_input_tokens: u64,
}

impl From<AnthropicUsage> for Usage {
    fn from(mut value: AnthropicUsage) -> Self {
        if let Some(output_details) = value
            .extra
            .get_mut("output_tokens_details")
            .and_then(Value::as_object_mut)
        {
            output_details.retain(|key, _| !is_internal_key(key));
        }
        retain_wire_extra_fields(&mut value.extra);
        let native_reasoning_tokens = value
            .extra
            .get_mut("output_tokens_details")
            .and_then(Value::as_object_mut)
            .and_then(|details| details.remove("thinking_tokens"))
            .and_then(|value| value_to_u64(&value));
        let output_tokens_details_is_empty = value
            .extra
            .get("output_tokens_details")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty);
        if output_tokens_details_is_empty {
            value.extra.remove("output_tokens_details");
        }
        let reasoning_tokens = native_reasoning_tokens.unwrap_or(value.reasoning_tokens);

        let input_details = (value.cache_read_input_tokens > 0
            || value.cache_creation_input_tokens > 0
            || value.tool_prompt_tokens > 0)
            .then_some(InputDetails {
                tool_prompt_modality_breakdown: None,
                standard_tokens: 0,
                cache_read_tokens: value.cache_read_input_tokens,
                cache_read_modality_breakdown: None,
                cache_creation_tokens: value.cache_creation_input_tokens,
                cache_creation_5m_tokens: value.cache_creation.ephemeral_5m_input_tokens,
                cache_creation_1h_tokens: value.cache_creation.ephemeral_1h_input_tokens,
                tool_prompt_tokens: value.tool_prompt_tokens,
                modality_breakdown: None,
            });

        let output_details = (reasoning_tokens > 0
            || value.accepted_prediction_tokens > 0
            || value.rejected_prediction_tokens > 0)
            .then_some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens,
                accepted_prediction_tokens: value.accepted_prediction_tokens,
                rejected_prediction_tokens: value.rejected_prediction_tokens,
                modality_breakdown: None,
            });

        // Wire `input_tokens` excludes the cache buckets; internal `input_tokens` includes them
        // (user-billing-and-model-metadata.spec.md § 5 C3-ii). Messages encoders subtract them back out.
        let normalized_input_tokens = value
            .input_tokens
            .saturating_add(value.cache_read_input_tokens)
            .saturating_add(value.cache_creation_input_tokens);

        Usage {
            iterations: crate::urp::usage::decode_messages_iterations(
                value.extra.remove("iterations").as_ref(),
            ),
            input_tokens: normalized_input_tokens,
            output_tokens: value.output_tokens,
            input_details,
            output_details,
            extra_body: value.extra,
        }
    }
}

pub(crate) fn decode_usage(value: &Value) -> Option<Usage> {
    serde_json::from_value::<AnthropicUsage>(value.clone())
        .ok()
        .map(Usage::from)
}

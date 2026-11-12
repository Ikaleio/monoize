//! Gemini `usageMetadata` decoding (PG6, PG6a). Response decoding, stream decoding, and stream
//! metering share this parser.

use crate::urp::decode::{deserialize_u64ish_default, retain_wire_extra_fields};
use crate::urp::{InputDetails, ModalityBreakdown, OutputDetails, Usage};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "prompt_token_count"
    )]
    prompt_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "candidates_token_count"
    )]
    candidates_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "thoughts_token_count",
        alias = "reasoning_tokens",
        alias = "reasoning_output_token_count"
    )]
    thoughts_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cached_content_token_count",
        alias = "cached_tokens",
        alias = "cache_read_tokens",
        alias = "cache_read_input_tokens"
    )]
    cached_content_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_creation_input_tokens",
        alias = "cache_write_tokens",
        alias = "cacheCreationTokenCount",
        alias = "cache_creation_token_count"
    )]
    cache_creation_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "toolUsePromptTokenCount",
        alias = "tool_use_prompt_token_count",
        alias = "toolPromptInputTokenCount",
        alias = "tool_prompt_input_token_count",
        alias = "tool_prompt_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "accepted_prediction_token_count",
        alias = "accepted_prediction_tokens",
        alias = "acceptedPredictionOutputTokenCount"
    )]
    accepted_prediction_token_count: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "rejected_prediction_token_count",
        alias = "rejected_prediction_tokens",
        alias = "rejectedPredictionOutputTokenCount"
    )]
    rejected_prediction_token_count: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl TryFrom<GeminiUsage> for Usage {
    type Error = String;

    fn try_from(mut value: GeminiUsage) -> Result<Self, Self::Error> {
        retain_wire_extra_fields(&mut value.extra);
        let input_modality = value
            .extra
            .remove("promptTokensDetails")
            .and_then(parse_modality);
        let cache_modality = value
            .extra
            .remove("cacheTokensDetails")
            .and_then(parse_modality);
        let output_modality = value
            .extra
            .remove("candidatesTokensDetails")
            .and_then(parse_modality);
        let tool_prompt_modality = value
            .extra
            .remove("toolUsePromptTokensDetails")
            .and_then(parse_modality);
        // Usage extras carry counters only; enum metadata such as `serviceTier` is not usage.
        value.extra.retain(|_, value| value.is_number());
        let input_tokens = value
            .prompt_token_count
            .checked_add(value.tool_prompt_tokens)
            .ok_or_else(|| "Gemini input token total overflow".to_string())?;
        let output_tokens = value
            .candidates_token_count
            .checked_add(value.thoughts_token_count)
            .ok_or_else(|| "Gemini output token total overflow".to_string())?;
        let input_details = (value.cached_content_token_count > 0
            || value.cache_creation_tokens > 0
            || value.tool_prompt_tokens > 0
            || input_modality.is_some()
            || cache_modality.is_some()
            || tool_prompt_modality.is_some())
        .then_some(InputDetails {
            standard_tokens: 0,
            cache_read_tokens: value.cached_content_token_count,
            cache_read_modality_breakdown: cache_modality,
            cache_creation_tokens: value.cache_creation_tokens,
            cache_creation_5m_tokens: 0,
            cache_creation_1h_tokens: 0,
            tool_prompt_tokens: value.tool_prompt_tokens,
            tool_prompt_modality_breakdown: tool_prompt_modality,
            modality_breakdown: input_modality,
        });
        let output_details = (value.thoughts_token_count > 0
            || value.accepted_prediction_token_count > 0
            || value.rejected_prediction_token_count > 0
            || output_modality.is_some())
        .then_some(OutputDetails {
            standard_tokens: 0,
            reasoning_tokens: value.thoughts_token_count,
            accepted_prediction_tokens: value.accepted_prediction_token_count,
            rejected_prediction_tokens: value.rejected_prediction_token_count,
            modality_breakdown: output_modality,
        });

        Ok(Usage {
            iterations: None,
            input_tokens,
            output_tokens,
            input_details,
            output_details,
            extra_body: value.extra,
        })
    }
}

fn parse_modality(value: Value) -> Option<ModalityBreakdown> {
    let mut result = ModalityBreakdown::default();
    for entry in value.as_array()? {
        let count = entry.get("tokenCount").and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        });
        match entry.get("modality").and_then(Value::as_str) {
            Some("TEXT") => result.text_tokens = count,
            Some("IMAGE") => result.image_tokens = count,
            Some("AUDIO") => result.audio_tokens = count,
            Some("VIDEO") => result.video_tokens = count,
            Some("DOCUMENT") => result.document_tokens = count,
            _ => {}
        }
    }
    Some(result)
}

/// Decodes one cumulative `usageMetadata` snapshot. `totalTokenCount` is derived, not stored.
pub(crate) fn parse_usage(obj: &Map<String, Value>) -> Result<Usage, String> {
    let mut native = obj.clone();
    native.remove("totalTokenCount");
    serde_json::from_value::<GeminiUsage>(Value::Object(native))
        .map_err(|err| format!("invalid Gemini usage metadata: {err}"))?
        .try_into()
}

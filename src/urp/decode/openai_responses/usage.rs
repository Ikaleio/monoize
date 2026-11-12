use crate::urp::decode::{deserialize_u64ish_default, retain_wire_extra_fields, split_extra};
use crate::urp::{InputDetails, OutputDetails, Usage};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesUsage {
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
    #[serde(default)]
    input_tokens_details: Option<OpenAiResponsesInputDetails>,
    #[serde(default)]
    output_tokens_details: Option<OpenAiResponsesOutputDetails>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiResponsesInputDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiResponsesOutputDetails>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesInputDetails {
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens"
    )]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "tool_prompt_input_tokens"
    )]
    tool_prompt_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponsesOutputDetails {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
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

impl From<OpenAiResponsesUsage> for Usage {
    fn from(value: OpenAiResponsesUsage) -> Self {
        let OpenAiResponsesUsage {
            input_tokens,
            output_tokens,
            mut input_tokens_details,
            mut output_tokens_details,
            mut prompt_tokens_details,
            mut completion_tokens_details,
            mut extra,
        } = value;

        retain_wire_extra_fields(&mut extra);
        for details in [&mut input_tokens_details, &mut prompt_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }
        for details in [&mut output_tokens_details, &mut completion_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }

        let input_details = input_tokens_details
            .as_ref()
            .or(prompt_tokens_details.as_ref())
            .and_then(|details| {
                let cache_creation_tokens = details
                    .cache_creation_tokens
                    .max(details.cache_write_tokens);
                (details.cached_tokens > 0
                    || cache_creation_tokens > 0
                    || details.tool_prompt_tokens > 0)
                    .then(|| InputDetails {
                        tool_prompt_modality_breakdown: None,
                        standard_tokens: 0,
                        cache_read_tokens: details.cached_tokens,
                        cache_read_modality_breakdown: None,
                        cache_creation_tokens,
                        cache_creation_5m_tokens: 0,
                        cache_creation_1h_tokens: 0,
                        tool_prompt_tokens: details.tool_prompt_tokens,
                        modality_breakdown: None,
                    })
            });

        let output_details = output_tokens_details
            .as_ref()
            .or(completion_tokens_details.as_ref())
            .and_then(|details| {
                (details.reasoning_tokens > 0
                    || details.accepted_prediction_tokens > 0
                    || details.rejected_prediction_tokens > 0)
                    .then(|| OutputDetails {
                        standard_tokens: 0,
                        reasoning_tokens: details.reasoning_tokens,
                        accepted_prediction_tokens: details.accepted_prediction_tokens,
                        rejected_prediction_tokens: details.rejected_prediction_tokens,
                        modality_breakdown: None,
                    })
            });

        let detail_extras = [
            (
                "input_tokens_details",
                input_tokens_details.map(|d| d.extra),
            ),
            (
                "prompt_tokens_details",
                prompt_tokens_details.map(|d| d.extra),
            ),
            (
                "output_tokens_details",
                output_tokens_details.map(|d| d.extra),
            ),
            (
                "completion_tokens_details",
                completion_tokens_details.map(|d| d.extra),
            ),
        ];
        for (key, details_extra) in detail_extras {
            if let Some(details_extra) = details_extra
                && !details_extra.is_empty()
            {
                extra.insert(
                    key.to_string(),
                    Value::Object(details_extra.into_iter().collect()),
                );
            }
        }

        Usage {
            iterations: None,
            input_tokens,
            output_tokens,
            input_details,
            output_details,
            extra_body: extra,
        }
    }
}

pub(crate) fn parse_usage_from_responses(obj: &Map<String, Value>) -> Usage {
    serde_json::from_value::<OpenAiResponsesUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            iterations: None,
            input_tokens: 0,
            output_tokens: 0,
            input_details: None,
            output_details: None,
            extra_body: split_extra(obj, &[]),
        })
}

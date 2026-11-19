use crate::urp::decode::{
    deserialize_u64ish_default, deserialize_u64ish_optional, retain_wire_extra_fields, split_extra,
    value_to_u64,
};
use crate::urp::{InputDetails, OutputDetails, Usage};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatUsage {
    #[serde(default, deserialize_with = "deserialize_u64ish_optional")]
    prompt_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_u64ish_optional")]
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiChatInputDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiChatOutputDetails>,
    #[serde(default)]
    input_tokens_details: Option<OpenAiChatInputDetails>,
    #[serde(default)]
    output_tokens_details: Option<OpenAiChatOutputDetails>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatInputDetails {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
    #[serde(default)]
    cache_write_tokens_details: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    tool_prompt_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatOutputDetails {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    reasoning_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    accepted_prediction_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    rejected_prediction_tokens: u64,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

fn detail_extra_object(extra: &HashMap<String, Value>) -> Map<String, Value> {
    extra
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn input_details(details: &OpenAiChatInputDetails) -> Option<InputDetails> {
    let split =
        crate::urp::CacheWriteTokensDetails::from_wire(details.cache_write_tokens_details.as_ref());
    let cache_creation_tokens = details
        .cache_creation_tokens
        .max(details.cache_write_tokens)
        .max(split.as_ref().map_or(0, |s| s.total()));
    let extra = detail_extra_object(&details.extra);
    let modality_breakdown =
        crate::handlers::usage::parse_modality_breakdown_from_detail_object(Some(&extra));
    let cache_read_modality_breakdown =
        crate::handlers::usage::parse_cache_read_modality_breakdown_from_detail_object(Some(
            &extra,
        ));
    let reported = details.cached_tokens > 0
        || split.is_some()
        || cache_creation_tokens > 0
        || details.tool_prompt_tokens > 0
        || modality_breakdown.is_some()
        || cache_read_modality_breakdown.is_some();
    reported.then_some(InputDetails {
        tool_prompt_modality_breakdown: None,
        standard_tokens: 0,
        cache_read_tokens: details.cached_tokens,
        cache_read_modality_breakdown,
        cache_creation_tokens,
        cache_write_tokens_details: split,
        cache_creation_5m_tokens: 0,
        cache_creation_1h_tokens: 0,
        tool_prompt_tokens: details.tool_prompt_tokens,
        modality_breakdown,
    })
}

fn output_details(details: &OpenAiChatOutputDetails) -> Option<OutputDetails> {
    let extra = detail_extra_object(&details.extra);
    let modality_breakdown =
        crate::handlers::usage::parse_modality_breakdown_from_detail_object(Some(&extra));
    let reported = details.reasoning_tokens > 0
        || details.accepted_prediction_tokens > 0
        || details.rejected_prediction_tokens > 0
        || modality_breakdown.is_some();
    reported.then_some(OutputDetails {
        standard_tokens: 0,
        reasoning_tokens: details.reasoning_tokens,
        accepted_prediction_tokens: details.accepted_prediction_tokens,
        rejected_prediction_tokens: details.rejected_prediction_tokens,
        modality_breakdown,
    })
}

/// Keeps the unknown members of a details object as wire passthrough under its own key.
fn retain_detail_extra(
    extra: &mut HashMap<String, Value>,
    key: &str,
    mut detail_extra: HashMap<String, Value>,
) {
    if detail_extra.is_empty() {
        return;
    }
    crate::urp::usage::strip_modality(&mut detail_extra);
    extra.insert(
        key.to_string(),
        Value::Object(detail_extra.into_iter().collect()),
    );
}

impl From<OpenAiChatUsage> for Usage {
    fn from(value: OpenAiChatUsage) -> Self {
        let OpenAiChatUsage {
            prompt_tokens,
            completion_tokens,
            mut prompt_tokens_details,
            mut completion_tokens_details,
            mut input_tokens_details,
            mut output_tokens_details,
            mut extra,
        } = value;

        retain_wire_extra_fields(&mut extra);
        for details in [&mut prompt_tokens_details, &mut input_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }
        for details in [&mut completion_tokens_details, &mut output_tokens_details]
            .into_iter()
            .flatten()
        {
            retain_wire_extra_fields(&mut details.extra);
        }

        let input_details = prompt_tokens_details
            .as_ref()
            .or(input_tokens_details.as_ref())
            .and_then(input_details);
        let output_details = completion_tokens_details
            .as_ref()
            .or(output_tokens_details.as_ref())
            .and_then(output_details);

        for (key, details) in [
            ("prompt_tokens_details", prompt_tokens_details),
            ("input_tokens_details", input_tokens_details),
        ] {
            if let Some(details) = details {
                retain_detail_extra(&mut extra, key, details.extra);
            }
        }
        for (key, details) in [
            ("completion_tokens_details", completion_tokens_details),
            ("output_tokens_details", output_tokens_details),
        ] {
            if let Some(details) = details {
                retain_detail_extra(&mut extra, key, details.extra);
            }
        }

        Usage {
            iterations: None,
            reported_token_counts: prompt_tokens.is_some() && completion_tokens.is_some(),
            input_tokens: prompt_tokens.unwrap_or(0),
            output_tokens: completion_tokens.unwrap_or(0),
            input_details,
            output_details,
            extra_body: extra,
        }
    }
}

/// PC9a: the single Chat usage parser for non-stream responses and stream chunks.
pub(crate) fn parse_chat_usage(obj: &Map<String, Value>) -> Usage {
    let mut obj = obj.clone();
    for key in ["cost", "cost_details", "is_byok"] {
        obj.remove(key);
    }
    for (alias, canonical) in [
        ("input_tokens", "prompt_tokens"),
        ("output_tokens", "completion_tokens"),
    ] {
        if let Some(value) = obj.remove(alias)
            && !obj.contains_key(canonical)
        {
            obj.insert(canonical.to_string(), value);
        }
    }
    super::super::normalize_openai_usage_details(&mut obj);
    serde_json::from_value::<OpenAiChatUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| {
            let input_tokens = obj.get("prompt_tokens").and_then(value_to_u64);
            let output_tokens = obj.get("completion_tokens").and_then(value_to_u64);
            Usage {
                iterations: None,
                reported_token_counts: input_tokens.is_some() && output_tokens.is_some(),
                input_tokens: input_tokens.unwrap_or(0),
                output_tokens: output_tokens.unwrap_or(0),
                input_details: None,
                output_details: None,
                extra_body: split_extra(&obj, &[]),
            }
        })
}

use crate::urp::decode::{
    deserialize_u64ish_default, retain_wire_extra_fields, split_extra, value_to_u64,
};
use crate::urp::{InputDetails, OutputDetails, Usage};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatUsage {
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    prompt_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    completion_tokens: u64,
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
    #[serde(
        default,
        deserialize_with = "deserialize_u64ish_default",
        alias = "cache_read_tokens"
    )]
    cached_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_u64ish_default")]
    cache_creation_tokens: u64,
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
struct OpenAiChatOutputDetails {
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

fn detail_extra_object(extra: &HashMap<String, Value>) -> Map<String, Value> {
    extra
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn input_details(details: &OpenAiChatInputDetails) -> Option<InputDetails> {
    let cache_creation_tokens = details
        .cache_creation_tokens
        .max(details.cache_write_tokens);
    let extra = detail_extra_object(&details.extra);
    let modality_breakdown =
        crate::handlers::usage::parse_modality_breakdown_from_detail_object(Some(&extra));
    let cache_read_modality_breakdown =
        crate::handlers::usage::parse_cache_read_modality_breakdown_from_detail_object(Some(
            &extra,
        ));
    let reported = details.cached_tokens > 0
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
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
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
    for (key, aliases) in [
        ("prompt_tokens_details", INPUT_DETAIL_ALIASES),
        ("input_tokens_details", INPUT_DETAIL_ALIASES),
        ("completion_tokens_details", OUTPUT_DETAIL_ALIASES),
        ("output_tokens_details", OUTPUT_DETAIL_ALIASES),
    ] {
        match obj.get_mut(key) {
            Some(Value::Object(details)) => canonicalize_usage_detail_aliases(details, aliases),
            Some(_) => {
                obj.remove(key);
            }
            None => {}
        }
    }
    serde_json::from_value::<OpenAiChatUsage>(Value::Object(obj.clone()))
        .map(Usage::from)
        .unwrap_or_else(|_| Usage {
            iterations: None,
            input_tokens: obj.get("prompt_tokens").and_then(value_to_u64).unwrap_or(0),
            output_tokens: obj
                .get("completion_tokens")
                .and_then(value_to_u64)
                .unwrap_or(0),
            input_details: None,
            output_details: None,
            extra_body: split_extra(&obj, &[]),
        })
}

const INPUT_DETAIL_ALIASES: &[(&str, &str)] = &[
    ("cache_read_tokens", "cached_tokens"),
    ("tool_prompt_input_tokens", "tool_prompt_tokens"),
];
const OUTPUT_DETAIL_ALIASES: &[(&str, &str)] = &[
    (
        "accepted_prediction_output_tokens",
        "accepted_prediction_tokens",
    ),
    (
        "rejected_prediction_output_tokens",
        "rejected_prediction_tokens",
    ),
];

/// PC9a: an alias next to its canonical field is a serde duplicate-field error, which must not
/// discard the rest of the usage. The larger counter wins.
fn canonicalize_usage_detail_aliases(details: &mut Map<String, Value>, aliases: &[(&str, &str)]) {
    for (alias, canonical) in aliases {
        let Some(alias_value) = details.remove(*alias) else {
            continue;
        };
        let keep_alias = match details.get(*canonical) {
            Some(existing) => {
                value_to_u64(&alias_value).unwrap_or(0) > value_to_u64(existing).unwrap_or(0)
            }
            None => true,
        };
        if keep_alias {
            details.insert((*canonical).to_string(), alias_value);
        }
    }
}

//! Gemini `generationConfig` decoding (GEM-4, GEM-4a): typed controls, and the native members
//! that remain request extras. The encoder shares the typed-member lists, so typed values and
//! their deletion always win over retained native members.

use super::schema::native_schema_types;
use crate::urp::{
    GEMINI_ENUM_FORMAT_KEY, JsonSchemaDefinition, LogprobConfig, ReasoningConfig, ResponseFormat,
    SamplingConfig, StopControl,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// `generationConfig` members owned by typed request fields.
pub(crate) const TYPED_GENERATION_KEYS: [&str; 10] = [
    "temperature",
    "topP",
    "maxOutputTokens",
    "stopSequences",
    "responseLogprobs",
    "logprobs",
    "topK",
    "seed",
    "presencePenalty",
    "frequencyPenalty",
];

/// `thinkingConfig` members owned by `ReasoningConfig`.
pub(crate) const TYPED_THINKING_KEYS: [&str; 3] =
    ["thinkingLevel", "thinkingBudget", "includeThoughts"];

/// Legacy response-format members owned by `ResponseFormat`.
pub(crate) const RESPONSE_SCHEMA_KEYS: [&str; 3] =
    ["responseMimeType", "responseJsonSchema", "responseSchema"];

/// Whether a legacy `responseMimeType` is owned by `ResponseFormat`.
pub(crate) fn is_typed_response_mime(config: &Map<String, Value>) -> bool {
    matches!(
        config.get("responseMimeType").and_then(Value::as_str),
        Some("text/plain" | "application/json" | "text/x.enum")
    )
}

/// Typed request controls of one `generationConfig`.
pub(super) struct GenerationControls {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_output_tokens: Option<u64>,
    pub stop: Option<StopControl>,
    pub sampling: Option<SamplingConfig>,
    pub logprobs: Option<LogprobConfig>,
    pub reasoning: Option<ReasoningConfig>,
    pub response_format: Option<ResponseFormat>,
}

impl GenerationControls {
    pub(super) fn decode(config: Option<&Map<String, Value>>) -> Self {
        let Some(config) = config else {
            return Self {
                temperature: None,
                top_p: None,
                max_output_tokens: None,
                stop: None,
                sampling: None,
                logprobs: None,
                reasoning: None,
                response_format: None,
            };
        };
        Self {
            temperature: config.get("temperature").and_then(Value::as_f64),
            top_p: config.get("topP").and_then(Value::as_f64),
            max_output_tokens: config.get("maxOutputTokens").and_then(Value::as_u64),
            stop: config
                .get("stopSequences")
                .cloned()
                .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
                .map(StopControl::Multiple),
            sampling: decode_sampling(config),
            logprobs: decode_logprobs(config),
            reasoning: config
                .get("thinkingConfig")
                .and_then(Value::as_object)
                .map(decode_thinking),
            response_format: decode_response_format(config),
        }
    }
}

fn decode_sampling(config: &Map<String, Value>) -> Option<SamplingConfig> {
    let sampling = SamplingConfig {
        top_k: config
            .get("topK")
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok()),
        seed: config.get("seed").and_then(Value::as_i64),
        presence_penalty: config.get("presencePenalty").and_then(Value::as_f64),
        frequency_penalty: config.get("frequencyPenalty").and_then(Value::as_f64),
    };
    (sampling.top_k.is_some()
        || sampling.seed.is_some()
        || sampling.presence_penalty.is_some()
        || sampling.frequency_penalty.is_some())
    .then_some(sampling)
}

fn decode_logprobs(config: &Map<String, Value>) -> Option<LogprobConfig> {
    let enabled = config.get("responseLogprobs").and_then(Value::as_bool);
    let top_k = config
        .get("logprobs")
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok());
    enabled
        .or(top_k.map(|_| true))
        .map(|enabled| LogprobConfig { enabled, top_k })
}

fn decode_thinking(config: &Map<String, Value>) -> ReasoningConfig {
    ReasoningConfig {
        effort: config
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .filter(|effort| !effort.eq_ignore_ascii_case("THINKING_LEVEL_UNSPECIFIED"))
            .map(str::to_ascii_lowercase),
        budget_tokens: config.get("thinkingBudget").and_then(Value::as_u64),
        mode: config
            .get("thinkingBudget")
            .and_then(Value::as_i64)
            .and_then(|budget| match budget {
                0 => Some("disabled".into()),
                -1 => Some("adaptive".into()),
                _ => None,
            }),
        summary: config
            .get("includeThoughts")
            .and_then(Value::as_bool)
            .map(|enabled| if enabled { "auto" } else { "none" }.into()),
        ..Default::default()
    }
}

fn decode_response_format(config: &Map<String, Value>) -> Option<ResponseFormat> {
    let modern = config
        .get("responseFormat")
        .and_then(|format| format.get("text"));
    let mime = modern
        .and_then(|format| format.get("mimeType"))
        .and_then(Value::as_str)
        .or_else(|| config.get("responseMimeType").and_then(Value::as_str))?;
    match mime {
        "text/plain" | "TEXT_PLAIN" => return Some(ResponseFormat::Text),
        "application/json" | "APPLICATION_JSON" | "text/x.enum" => {}
        _ => return None,
    }
    let enum_format = mime == "text/x.enum";
    let json_schema = modern
        .and_then(|format| format.get("schema"))
        .or_else(|| config.get("responseJsonSchema"));
    let schema = match (json_schema, config.get("responseSchema")) {
        (Some(schema), _) => schema.clone(),
        (None, Some(native)) => native_schema_types(native, false),
        (None, None) if enum_format => json!({"type":"string"}),
        (None, None) => return Some(ResponseFormat::JsonObject),
    };
    let mut extra_body = HashMap::new();
    if enum_format {
        extra_body.insert(GEMINI_ENUM_FORMAT_KEY.into(), json!(true));
    }
    Some(ResponseFormat::JsonSchema {
        json_schema: JsonSchemaDefinition {
            name: "gemini_response".into(),
            description: None,
            schema,
            strict: None,
            extra_body,
        },
    })
}

/// Whether the request selected its output format through the modern `responseFormat.text`.
pub(super) fn uses_text_response_format(config: &Map<String, Value>) -> bool {
    config
        .get("responseFormat")
        .and_then(|format| format.get("text"))
        .and_then(|text| text.get("mimeType"))
        .and_then(Value::as_str)
        .is_some_and(|mime| matches!(mime, "TEXT_PLAIN" | "APPLICATION_JSON"))
}

/// Removes the members that typed request fields own from a retained `generationConfig`.
pub(super) fn strip_typed_generation_config(config: &mut Map<String, Value>) {
    for key in TYPED_GENERATION_KEYS {
        config.remove(key);
    }
    strip_text_response_format(config);
    if is_typed_response_mime(config) {
        for key in RESPONSE_SCHEMA_KEYS {
            config.remove(key);
        }
    }
    if let Some(thinking) = config
        .get_mut("thinkingConfig")
        .and_then(Value::as_object_mut)
    {
        for key in TYPED_THINKING_KEYS {
            thinking.remove(key);
        }
    }
}

/// Removes the typed members of `responseFormat.text` and the containers they leave empty.
pub(crate) fn strip_text_response_format(config: &mut Map<String, Value>) {
    let Some(format) = config
        .get_mut("responseFormat")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(text) = format.get_mut("text").and_then(Value::as_object_mut) {
        if text
            .get("mimeType")
            .and_then(Value::as_str)
            .is_some_and(|mime| matches!(mime, "TEXT_PLAIN" | "APPLICATION_JSON" | "text/x.enum"))
        {
            text.remove("mimeType");
            text.remove("schema");
        }
        if text.is_empty() {
            format.remove("text");
        }
    }
    if format.is_empty() {
        config.remove("responseFormat");
    }
}

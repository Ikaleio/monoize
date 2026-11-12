//! `generationConfig` encoding (PG4c, PG4i, GEM-4, GEM-4a): native passthrough members with
//! every typed control overlaid. A typed value, including its absence, wins over a retained
//! native member.

use super::model::GeminiModel;
use super::object_member;
use crate::urp::decode::gemini::{
    RESPONSE_SCHEMA_KEYS, TYPED_GENERATION_KEYS, TYPED_THINKING_KEYS, is_typed_response_mime,
    native_schema_types, strip_text_response_format,
};
use crate::urp::{
    GEMINI_ENUM_FORMAT_KEY, GEMINI_TEXT_FORMAT_KEY, ProviderProtocol, ResponseFormat, StopControl,
    UrpRequest,
};
use serde_json::{Map, Value, json};

pub(super) fn generation_config(req: &UrpRequest, model: &GeminiModel) -> Map<String, Value> {
    let mut config = req
        .extra_body
        .get("generationConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in TYPED_GENERATION_KEYS {
        config.remove(key);
    }
    strip_text_response_format(&mut config);
    if let Some(sampling) = &req.sampling {
        for (key, value) in [
            ("topK", sampling.top_k.map(Value::from)),
            ("seed", sampling.seed.map(Value::from)),
            (
                "presencePenalty",
                sampling.presence_penalty.map(Value::from),
            ),
            (
                "frequencyPenalty",
                sampling.frequency_penalty.map(Value::from),
            ),
        ] {
            if let Some(value) = value {
                config.insert(key.into(), value);
            }
        }
    }
    if let Some(logprobs) = &req.logprobs {
        config.insert("responseLogprobs".into(), json!(logprobs.enabled));
        if logprobs.enabled
            && let Some(count) = logprobs.top_k
        {
            config.insert("logprobs".into(), json!(count));
        }
    }
    if req.response_format.is_some() || is_typed_response_mime(&config) {
        for key in RESPONSE_SCHEMA_KEYS {
            config.remove(key);
        }
    }
    if let Some(temperature) = req.temperature {
        config.insert("temperature".into(), Value::from(temperature));
    }
    if let Some(top_p) = req.top_p {
        config.insert("topP".into(), Value::from(top_p));
    }
    if let Some(max_tokens) = req.max_output_tokens {
        config.insert("maxOutputTokens".into(), Value::from(max_tokens));
    }
    if let Some(stop) = &req.stop {
        config.insert(
            "stopSequences".into(),
            match stop {
                StopControl::Single(stop) => json!([stop]),
                StopControl::Multiple(stops) => json!(stops),
            },
        );
    }
    if let Some(format) = &req.response_format {
        let text_format = req
            .extra_body
            .get(GEMINI_TEXT_FORMAT_KEY)
            .and_then(Value::as_bool)
            == Some(true);
        insert_response_format(&mut config, format, text_format);
    }
    let mut thinking = config
        .remove("thinkingConfig")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    for key in TYPED_THINKING_KEYS {
        thinking.remove(key);
    }
    if let Some(reasoning) = &req.reasoning {
        thinking.extend(model.thinking_config(reasoning));
    }
    if !thinking.is_empty() {
        config.insert("thinkingConfig".into(), Value::Object(thinking));
    }
    insert_image_output(&mut config, req, model);
    config
}

/// Legacy `responseMimeType` placement, or the modern `responseFormat.text` placement when the
/// source request used it. Legacy `text/x.enum` always uses the native `responseSchema`.
fn insert_response_format(
    config: &mut Map<String, Value>,
    format: &ResponseFormat,
    text_format: bool,
) {
    let schema = match format {
        ResponseFormat::JsonSchema { json_schema } => Some(json_schema),
        _ => None,
    };
    let enum_format = schema.is_some_and(|schema| {
        schema
            .extra_body
            .get(GEMINI_ENUM_FORMAT_KEY)
            .and_then(Value::as_bool)
            == Some(true)
    });
    if text_format && !enum_format {
        let native = object_member(config, "responseFormat");
        let text = object_member(native, "text");
        text.insert(
            "mimeType".into(),
            json!(if matches!(format, ResponseFormat::Text) {
                "TEXT_PLAIN"
            } else {
                "APPLICATION_JSON"
            }),
        );
        if let Some(schema) = schema {
            text.insert("schema".into(), schema.schema.clone());
        }
        return;
    }
    config.insert(
        "responseMimeType".into(),
        json!(if matches!(format, ResponseFormat::Text) {
            "text/plain"
        } else if enum_format {
            "text/x.enum"
        } else {
            "application/json"
        }),
    );
    if let Some(schema) = schema {
        if enum_format {
            config.insert(
                "responseSchema".into(),
                native_schema_types(&schema.schema, true),
            );
        } else {
            config.insert("responseJsonSchema".into(), schema.schema.clone());
        }
    }
}

/// PG4i: explicit Images requests always ask for image output; a declared `image_generation`
/// tool applies only to image-capable models and is otherwise a dropped definition.
fn insert_image_output(config: &mut Map<String, Value>, req: &UrpRequest, model: &GeminiModel) {
    let image_tool = req.tools.as_deref().and_then(|tools| {
        tools.iter().find(|tool| {
            tool.origin_protocol != Some(ProviderProtocol::Gemini)
                && tool.tool_type == "image_generation"
        })
    });
    let size = match (&req.image_generation, image_tool) {
        (Some(options), _) => options.size.clone(),
        (None, Some(tool)) if model.image_output => tool
            .config
            .as_ref()
            .and_then(|config| config.get("size"))
            .or_else(|| tool.extra_body.get("size"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => return,
    };
    config
        .entry("responseModalities")
        .or_insert_with(|| json!(["TEXT", "IMAGE"]));
    if let Some(image_config) = size.as_deref().and_then(|size| model.image_config(size)) {
        let native = object_member(config, "imageConfig");
        for (key, value) in image_config {
            native.entry(key).or_insert(value);
        }
    }
}

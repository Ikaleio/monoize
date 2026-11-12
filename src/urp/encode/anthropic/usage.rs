use crate::urp::encode::{usage_input_details, usage_output_details};
use crate::urp::{Usage, is_internal_key};
use serde_json::{Map, Value, json};

/// Inverts the decoder's normalization: URP `input_tokens` is inclusive (spec § 5 C3), while
/// Messages `input_tokens` excludes the cache read and cache creation buckets. Saturating
/// subtraction guards against upstream data whose cache buckets exceed the recorded total.
pub(crate) fn anthropic_native_input_tokens(usage: &Usage) -> u64 {
    let (cache_read, cache_creation) = usage.input_details.as_ref().map_or((0, 0), |details| {
        (details.cache_read_tokens, details.cache_creation_tokens)
    });
    usage
        .input_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_creation)
}

pub(crate) fn anthropic_native_usage_json(usage: &Usage) -> Value {
    let input_details = usage_input_details(usage);
    let output_details = usage_output_details(usage);
    let mut usage_object: Map<String, Value> = usage
        .extra_body
        .iter()
        .filter(|(key, _)| !is_internal_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    usage_object.insert(
        "input_tokens".to_string(),
        Value::from(anthropic_native_input_tokens(usage)),
    );
    usage_object.insert(
        "output_tokens".to_string(),
        Value::from(usage.output_tokens),
    );
    usage_object.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(input_details.cache_read_tokens),
    );
    usage_object.insert(
        "cache_creation_input_tokens".to_string(),
        Value::from(input_details.cache_creation_tokens),
    );
    for (key, value) in [
        ("tool_prompt_input_tokens", input_details.tool_prompt_tokens),
        (
            "accepted_prediction_output_tokens",
            output_details.accepted_prediction_tokens,
        ),
        (
            "rejected_prediction_output_tokens",
            output_details.rejected_prediction_tokens,
        ),
    ] {
        usage_object.remove(key);
        if value > 0 {
            usage_object.insert(key.to_string(), Value::from(value));
        }
    }
    usage_object.remove("reasoning_output_tokens");
    let mut native_output_details = take_object(&mut usage_object, "output_tokens_details");
    native_output_details.insert(
        "thinking_tokens".to_string(),
        Value::from(output_details.reasoning_tokens),
    );
    usage_object.insert(
        "output_tokens_details".to_string(),
        Value::Object(native_output_details),
    );

    let mut cache_creation = take_object(&mut usage_object, "cache_creation");
    cache_creation.insert(
        "ephemeral_5m_input_tokens".to_string(),
        Value::from(input_details.cache_creation_5m_tokens),
    );
    cache_creation.insert(
        "ephemeral_1h_input_tokens".to_string(),
        Value::from(input_details.cache_creation_1h_tokens),
    );
    usage_object.insert("cache_creation".to_string(), Value::Object(cache_creation));

    usage_object.remove("iterations");
    if let Some(iterations) = &usage.iterations {
        usage_object.insert(
            "iterations".into(),
            Value::Array(
                iterations
                    .iter()
                    .map(|iteration| {
                        let mut value = anthropic_native_usage_json(&iteration.usage);
                        value["type"] = json!(iteration.kind);
                        value
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(usage_object)
}

/// Removes a nested passthrough object, dropping internal keys and non-object values.
fn take_object(object: &mut Map<String, Value>, key: &str) -> Map<String, Value> {
    let mut nested = match object.remove(key) {
        Some(Value::Object(nested)) => nested,
        _ => Map::new(),
    };
    nested.retain(|key, _| !is_internal_key(key));
    nested
}

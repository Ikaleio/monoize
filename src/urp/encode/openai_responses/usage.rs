//! Responses `usage` objects.

use crate::urp::encode::{usage_input_details, usage_output_details};
use crate::urp::{Usage, is_internal_key};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(crate) fn encode_usage(usage: &Usage) -> Value {
    let aggregate = usage.accounting();
    let usage = aggregate.as_ref();
    let input_details = usage_input_details(usage);
    let output_details = usage_output_details(usage);
    let mut usage_value = json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens(),
        "output_tokens_details": {
            "reasoning_tokens": output_details.reasoning_tokens,
        },
        "input_tokens_details": {
            "cached_tokens": input_details.cache_read_tokens,
            "cache_write_tokens": input_details.cache_creation_tokens,
        }
    });
    // STR3s: non-native counters exist on the wire only when they carry a value.
    for (detail, key, value) in [
        (
            "input_tokens_details",
            "cache_creation_tokens",
            input_details.cache_creation_tokens,
        ),
        (
            "input_tokens_details",
            "tool_prompt_tokens",
            input_details.tool_prompt_tokens,
        ),
        (
            "output_tokens_details",
            "accepted_prediction_tokens",
            output_details.accepted_prediction_tokens,
        ),
        (
            "output_tokens_details",
            "rejected_prediction_tokens",
            output_details.rejected_prediction_tokens,
        ),
    ] {
        if value != 0 {
            usage_value[detail][key] = json!(value);
        }
    }
    merge_usage_extra(&mut usage_value, &usage.extra_body);
    usage_value
}

/// Passes unknown usage members through; generated detail counters win over extras.
fn merge_usage_extra(usage: &mut Value, extra: &HashMap<String, Value>) {
    let Some(usage_obj) = usage.as_object_mut() else {
        return;
    };

    for detail_key in ["input_tokens_details", "output_tokens_details"] {
        let Some(extra_detail) = extra.get(detail_key).and_then(Value::as_object) else {
            continue;
        };
        let Some(generated_detail) = usage_obj.get_mut(detail_key).and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (key, value) in extra_detail {
            if !is_internal_key(key) {
                generated_detail
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    for (key, value) in extra {
        if !is_internal_key(key)
            && !matches!(
                key.as_str(),
                "input_tokens_details" | "output_tokens_details"
            )
        {
            usage_obj.insert(key.clone(), value.clone());
        }
    }
}

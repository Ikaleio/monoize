use crate::urp::encode::{usage_input_details, usage_output_details};
use crate::urp::{Usage, is_internal_key};
use serde_json::{Map, Value, json};

/// DC7a: the Chat `usage` object for non-stream responses and the stream usage chunk.
pub(crate) fn encode_chat_usage(usage: &Usage) -> Value {
    let aggregate = usage.accounting();
    let usage = aggregate.as_ref();
    let input_details = usage_input_details(usage);
    let output_details = usage_output_details(usage);
    let mut prompt_details = Map::from_iter([(
        "cached_tokens".to_string(),
        json!(input_details.cache_read_tokens),
    )]);
    // Decision 4: counters outside the OpenAI schema appear only when non-zero.
    for (key, value) in [
        ("cache_write_tokens", input_details.cache_creation_tokens),
        ("cache_creation_tokens", input_details.cache_creation_tokens),
        ("tool_prompt_tokens", input_details.tool_prompt_tokens),
    ] {
        if value > 0 {
            prompt_details.insert(key.to_string(), json!(value));
        }
    }
    let mut usage_value = json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens(),
        "completion_tokens_details": {
            "reasoning_tokens": output_details.reasoning_tokens,
            "accepted_prediction_tokens": output_details.accepted_prediction_tokens,
            "rejected_prediction_tokens": output_details.rejected_prediction_tokens
        },
        "prompt_tokens_details": prompt_details,
    });
    let usage_obj = usage_value.as_object_mut().expect("chat usage object");
    for detail_key in ["prompt_tokens_details", "completion_tokens_details"] {
        let Some(extra_detail) = usage.extra_body.get(detail_key).and_then(Value::as_object) else {
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
    for (key, value) in &usage.extra_body {
        if !is_internal_key(key)
            && !matches!(
                key.as_str(),
                "prompt_tokens_details"
                    | "completion_tokens_details"
                    | "input_tokens_details"
                    | "output_tokens_details"
                    | "cost"
                    | "cost_details"
                    | "is_byok"
            )
        {
            usage_obj
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }
    crate::urp::usage::write_modality(
        &mut usage_value["prompt_tokens_details"],
        &input_details.modality_breakdown,
    );
    crate::urp::usage::write_modality(
        &mut usage_value["completion_tokens_details"],
        &output_details.modality_breakdown,
    );
    usage_value
}

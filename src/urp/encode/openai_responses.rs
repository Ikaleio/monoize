use crate::urp::encode::{
    file_id_origin_matches, merge_extra, role_to_str, sanitize_provider_item_wire_body, text_parts,
    tool_choice_to_responses_value, usage_input_details, usage_output_details,
};
use crate::urp::internal_legacy_bridge::{Item, Part, Role, nodes_to_items};
use crate::urp::{
    FILE_ID_ORIGIN_OPENAI, FileSource, FinishReason, ImageSource, Node, ProviderProtocol,
    REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY,
    RESPONSES_INSTRUCTION_NODE_EXTRA_KEY, RESPONSES_INSTRUCTIONS_EXTRA_KEY,
    RESPONSES_REASONING_CONTENT_EXTRA_KEY, RESPONSES_REASONING_SUMMARY_EXTRA_KEY,
    RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ResponseFormat, ToolCallType, ToolDefinition,
    ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

include!("openai_responses/message_items.inc.rs");
include!("openai_responses/reasoning.inc.rs");

fn merge_responses_usage_extra(usage: &mut Value, extra: &HashMap<String, Value>) {
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
            if !key.starts_with("_monoize_") {
                generated_detail
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    for (key, value) in extra {
        if !key.starts_with("_monoize_")
            && !matches!(
                key.as_str(),
                "input_tokens_details" | "output_tokens_details"
            )
        {
            usage_obj.insert(key.clone(), value.clone());
        }
    }
}

include!("openai_responses/tool_call.inc.rs");
include!("openai_responses/request_response.inc.rs");
include!("openai_responses/input_items.inc.rs");
include!("openai_responses/media.inc.rs");
include!("openai_responses/tools_format.inc.rs");
include!("openai_responses/tests.inc.rs");

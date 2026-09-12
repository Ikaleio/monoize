use crate::urp::encode::{
    merge_extra, sanitize_provider_item_wire_body, tool_choice_to_value, usage_input_details,
    usage_output_details,
};
use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, FileSource, FinishReason, ImageSource,
    MESSAGES_OUTPUT_CONFIG_EXTRA_KEY, MESSAGES_THINKING_CONFIG_EXTRA_KEY, MediaMetadata, Node,
    OrdinaryRole, ProviderProtocol, REASONING_ENVELOPE_PREFIX, ResponseFormat, StopControl,
    ToolCallType, ToolDefinition, ToolResultContent, UrpRequest, UrpResponse, Usage,
    strip_reasoning_signature_sigil, wrap_reasoning_signature_with_item_id,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

include!("anthropic/reasoning.inc.rs");
include!("anthropic/thinking_validation.inc.rs");
include!("anthropic/request_response.inc.rs");
include!("anthropic/messages_part1.inc.rs");
include!("anthropic/messages_part2.inc.rs");
include!("anthropic/tools.inc.rs");
include!("anthropic/media_config.inc.rs");

fn encode_messages_provider_block(
    origin_protocol: ProviderProtocol,
    id: Option<&Option<String>>,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::Messages {
        return None;
    }
    let sanitized_body = sanitize_provider_item_wire_body(body);
    let mut block = match sanitized_body {
        Value::Object(obj) => obj,
        _ => return None,
    };
    block.insert("type".to_string(), Value::String(item_type.to_string()));
    if block.contains_key("id")
        && let Some(id) = id
    {
        block.remove("id");
        if let Some(id) = id {
            block.insert("id".into(), Value::String(id.clone()));
        }
    }
    merge_extra(&mut block, extra_body);
    Some(Value::Object(block))
}

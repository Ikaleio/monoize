use crate::urp::encode::{
    merge_extra, tool_choice_to_value, usage_input_details, usage_output_details,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Node, OrdinaryRole, ProviderProtocol,
    REASONING_ENVELOPE_PREFIX, REASONING_KIND_EXTRA_KEY, REASONING_KIND_REDACTED_THINKING,
    ToolDefinition, ToolResultContent, UrpRequest, UrpResponse, Usage,
    strip_reasoning_signature_sigil, wrap_reasoning_signature_with_item_id,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

include!("anthropic/reasoning.inc.rs");
include!("anthropic/request_response.inc.rs");
include!("anthropic/messages_part1.inc.rs");
include!("anthropic/messages_part2.inc.rs");
include!("anthropic/tools.inc.rs");
include!("anthropic/media_config.inc.rs");
include!("anthropic/tests.inc.rs");

fn encode_messages_provider_block(
    origin_protocol: ProviderProtocol,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::Messages {
        return None;
    }
    let mut block = match body {
        Value::Object(obj) => obj.clone(),
        _ => return None,
    };
    block
        .entry("type".to_string())
        .or_insert_with(|| Value::String(item_type.to_string()));
    merge_extra(&mut block, extra_body);
    Some(Value::Object(block))
}

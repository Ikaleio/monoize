use crate::urp::encode::{
    merge_extra, tool_choice_to_value, usage_input_details, usage_output_details,
};
use crate::urp::{
    FileSource, FinishReason, ImageSource, Node, OrdinaryRole, REASONING_ENVELOPE_PREFIX,
    REASONING_KIND_EXTRA_KEY, REASONING_KIND_REDACTED_THINKING, ToolDefinition, ToolResultContent,
    UrpRequest, UrpResponse, Usage, strip_reasoning_signature_sigil,
    wrap_reasoning_signature_with_item_id,
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

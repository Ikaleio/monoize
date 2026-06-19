use crate::urp::encode::{
    merge_extra, role_to_str, text_parts, tool_choice_to_openai_value, usage_input_details,
    usage_output_details,
};
use crate::urp::internal_legacy_bridge::{Item, Part, Role, nodes_to_items};
use crate::urp::{
    FileSource, FinishReason, ImageSource, ProviderProtocol, ResponseFormat, ToolDefinition,
    ToolResultContent, UrpRequest, UrpResponse,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

include!("openai_responses/message_items.inc.rs");
include!("openai_responses/reasoning.inc.rs");
include!("openai_responses/tool_call.inc.rs");
include!("openai_responses/request_response.inc.rs");
include!("openai_responses/input_items.inc.rs");
include!("openai_responses/media.inc.rs");
include!("openai_responses/tools_format.inc.rs");
include!("openai_responses/tests.inc.rs");

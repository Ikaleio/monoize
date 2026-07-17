use crate::urp::encode::{
    merge_extra, role_to_str, sanitize_provider_item_wire_body, text_parts,
    tool_choice_to_openai_value, usage_input_details, usage_output_details,
};
use crate::urp::internal_legacy_bridge::{Item, Part, Role, nodes_to_items};
use crate::urp::{
    FileSource, FinishReason, ImageSource, ProviderProtocol,
    REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY, RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY,
    RESPONSES_REASONING_CONTENT_EXTRA_KEY, RESPONSES_REASONING_SUMMARY_EXTRA_KEY,
    RESPONSES_RESPONSE_SOURCE_EXTRA_KEY, ResponseFormat, ToolDefinition, ToolResultContent,
    UrpRequest, UrpResponse,
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

//! The single registry of internal adapter metadata names (XTRA-10, RSTRUCT-4).
//!
//! An `_monoize_` key lives in an `extra_body` map. It records native shape or
//! unknown native members and never owns a value that a typed URP field
//! represents. An `_monoize.` name identifies an internal `ProviderControl`
//! event or feeder frame. No encoder may emit either on a wire object.

/// Prefix of every internal `extra_body` key.
pub const INTERNAL_KEY_PREFIX: &str = "_monoize_";

/// Whether `key` is internal metadata rather than wire passthrough.
pub fn is_internal_key(key: &str) -> bool {
    key.starts_with(INTERNAL_KEY_PREFIX)
}

/// Prefix of every internal `ProviderControl` event name and feeder frame name.
pub const INTERNAL_CONTROL_PREFIX: &str = "_monoize.";

/// Whether `name` is an internal control rather than a native provider event.
pub fn is_internal_control(name: &str) -> bool {
    name.starts_with(INTERNAL_CONTROL_PREFIX)
}

// ---- Shared ----

/// Marks a document `File` whose native `content` was a string, not a block array.
pub const DOCUMENT_CONTENT_STRING_EXTRA_KEY: &str = "_monoize_document_content_string";

// ---- Chat Completions ----

/// Unknown entry fields and shape for one Chat reasoning detail, without semantic payload copies.
/// One source entry maps to one node so repeated entry types and entry order survive replay.
pub const CHAT_REASONING_DETAIL_EXTRA_KEY: &str = "_monoize_chat_reasoning_detail";
pub const CHAT_MESSAGE_ITEM_EXTRA_KEY: &str = "_monoize_chat_message_item";
/// Scalar Chat reasoning surface that supplied a reasoning node when no structured detail existed.
pub const CHAT_REASONING_SURFACE_EXTRA_KEY: &str = "_monoize_chat_reasoning_surface";
pub const CHAT_REASONING_SURFACE_REASONING: &str = "reasoning";
pub const CHAT_REASONING_SURFACE_REASONING_CONTENT: &str = "reasoning_content";
pub const CHAT_REASONING_SURFACE_REASONING_OPAQUE: &str = "reasoning_opaque";
/// Request field name (`max_tokens` or `max_completion_tokens`) a downstream Chat client used.
pub const CHAT_MAX_TOKENS_FIELD_EXTRA_KEY: &str = "_monoize_chat_max_tokens_field";
/// Unknown top-level fields of the first upstream Chat stream chunk, replayed on every
/// downstream Chat chunk envelope.
pub const CHAT_CHUNK_EXTRA_KEY: &str = "_monoize_chat_chunk_extra";
/// Unknown members of the selected Chat `choices[]` entry.
pub const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
/// Unknown members of one Chat stream `delta` object.
pub const CHAT_DELTA_EXTRA_BODY_KEY: &str = "_monoize_chat_delta_extra";
/// Native chunk-shaped error payload of a Chat stream.
pub const CHAT_ERROR_EVENT_EXTRA_KEY: &str = "_monoize_chat_error_event";
/// Native Chat `finish_reason` string; read only when the typed reason is `Other`.
pub const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";
/// Unknown provider-specific request controls and native configuration placement.
pub const CHAT_REASONING_CONFIG_EXTRA_KEY: &str = "_monoize_chat_reasoning_config";
pub const CHAT_THINKING_CONFIG_EXTRA_KEY: &str = "_monoize_chat_thinking_config";
pub const CHAT_MESSAGE_AUDIO_EXTRA_KEY: &str = "_monoize_chat_message_audio";
pub const CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY: &str =
    "_monoize_chat_legacy_function_definition";
pub const CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY: &str = "_monoize_chat_legacy_function_choice";
pub const CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY: &str = "_monoize_chat_legacy_function_call";
pub const CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY: &str = "_monoize_chat_legacy_function_result";
/// Message-level cache marker parked on the last carrier node of its Chat message until
/// request decode moves it to the typed field (CB-3a).
pub const CHAT_MESSAGE_MARKER_EXTRA_KEY: &str = "_monoize_chat_message_cache_marker";
/// Keeps the boundary of a Chat message that carried a message-level cache marker (CB-3a).
pub const CHAT_MESSAGE_BOUNDARY_EXTRA_KEY: &str = "_monoize_chat_message_cache_boundary";

// ---- Anthropic Messages ----

pub const MESSAGES_THINKING_CONFIG_EXTRA_KEY: &str = "_monoize_messages_thinking_config";
pub const MESSAGES_OUTPUT_CONFIG_EXTRA_KEY: &str = "_monoize_messages_output_config";
/// Native system-message boundary; typed nodes own the instruction content.
pub const MESSAGES_SYSTEM_ENVELOPE_EXTRA_KEY: &str = "_monoize_messages_system_envelope";
/// Chat reasoning-detail type of the node that opened a Messages thinking block (MSG-6).
pub const MESSAGES_CHAT_REASONING_DETAIL_TYPE_KEY: &str =
    "_monoize_messages_chat_reasoning_detail_type";

// ---- OpenAI Responses ----

/// Shape marker for a Responses image-generation item; typed source owns the image payload.
pub const RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY: &str =
    "_monoize_responses_image_generation_call";
/// Marks semantic nodes decoded from top-level Responses `instructions`.
pub const RESPONSES_INSTRUCTION_NODE_EXTRA_KEY: &str = "_monoize_responses_instruction_node";
/// Shape marker for a native Responses envelope; extras own unmodeled fields once.
pub const RESPONSES_RESPONSE_SOURCE_EXTRA_KEY: &str = "_monoize_responses_response_source";
/// Shape marker for a native Responses start envelope; no semantic snapshot is retained.
pub const RESPONSES_STREAM_START_SOURCE_EXTRA_KEY: &str = "_monoize_responses_stream_start_source";
/// Echo of the downstream request controls for a synthesized Responses object (STR3t).
pub const RESPONSES_REQUEST_ECHO_EXTRA_KEY: &str = "_monoize_responses_request_echo";
/// Marks a `ResponseDone` whose terminal snapshot discarded accumulated items (PR6c).
pub const RESPONSES_ITEMS_DISCARDED_EXTRA_KEY: &str = "_monoize_responses_items_discarded";
/// `phase` carried by an upstream `response.output_text.done` event, on `NodeDone.extra_body`.
pub const RESPONSES_TEXT_DONE_PHASE_EXTRA_KEY: &str = "_monoize_responses_text_done_phase";
/// Native shape of one Responses content part, without its typed payload.
pub const RESPONSES_CONTENT_PART_SHAPE_KEY: &str = "_monoize_responses_content_part";

// ---- Gemini GenerateContent ----

pub const GEMINI_PART_EXTRA_KEY: &str = "_monoize_gemini_part";
pub const GEMINI_CANDIDATE_EXTRA_KEY: &str = "_monoize_gemini_candidate";
pub const GEMINI_CONTENT_EXTRA_KEY: &str = "_monoize_gemini_content";
pub const GEMINI_SYSTEM_EXTRA_KEY: &str = "_monoize_gemini_system";
pub const GEMINI_TEXT_FORMAT_KEY: &str = "_monoize_gemini_text_response_format";
pub const GEMINI_ENUM_FORMAT_KEY: &str = "_monoize_gemini_enum_response";
pub const GEMINI_GROUNDING_EXTRA_KEY: &str = "_monoize_gemini_grounding";
pub const GEMINI_PARAMETERS_JSON_SCHEMA_EXTRA_KEY: &str = "_monoize_gemini_parameters_json_schema";
pub const GEMINI_RESPONSE_JSON_SCHEMA_EXTRA_KEY: &str = "_monoize_gemini_response_json_schema";
pub const GEMINI_FUNCTION_RESPONSE_EXTRA_KEY: &str = "_monoize_gemini_function_response";
pub const GEMINI_BOUND_SIGNATURE_KEY: &str = "_monoize_gemini_bound_signature";
pub const GEMINI_BOUND_OWNER_KEY: &str = "_monoize_gemini_bound_owner";
pub const GEMINI_BOUND_ITEM_OFFSET_KEY: &str = "_monoize_gemini_item_offset";

// ---- Internal control names ----

/// Chat-only stream control carrying annotations of a completed text segment (PC7h).
pub const CHAT_ANNOTATIONS_CONTROL: &str = "_monoize.chat.annotations";
/// Internal control carrying the object of a later upstream `response.in_progress`.
pub const RESPONSES_IN_PROGRESS_CONTROL: &str = "_monoize.responses.in_progress";
/// Internal control carrying the item-level fields of an upstream message `output_item.done`.
pub const RESPONSES_MESSAGE_DONE_CONTROL: &str = "_monoize.responses.message_done";
/// In-band feeder frame: data `true` when a `discard_partial_items` interrupt stands (PR6c).
pub const RESPONSES_DISCARD_REQUESTED_FRAME: &str = "_monoize.responses.discard_requested";

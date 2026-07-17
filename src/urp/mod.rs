use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub mod decode;
pub mod encode;
pub mod greedy;
pub(crate) mod internal_legacy_bridge;
pub mod stream_decode;
pub mod stream_encode;
pub mod stream_helpers;

pub fn synthetic_message_id() -> String {
    format!("msg_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_reasoning_id() -> String {
    format!("rs_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_tool_call_id() -> String {
    format!("fc_urp_{}", uuid::Uuid::new_v4().simple())
}

/// Prefix for the signature sigil used to smuggle a reasoning item id through an
/// Anthropic `thinking` / `redacted_thinking` block when the downstream client (Claude Code, etc.)
/// is known to strip unknown content-block fields. Format:
///   `mz1.<item_id>.<original_signature>`
/// The sigil is only attached when encoding reasoning toward the **downstream** (response encode).
/// When encoding toward an **upstream** request, a sigil-encoded signature is unwrapped so that the
/// upstream receives only the original opaque payload. See
/// `spec/unified_responses_proxy.spec.md` DM5.2 / PM5b.
pub const REASONING_SIGNATURE_SIGIL_PREFIX: &str = "mz1.";

/// Marker stored in `Node::Reasoning.extra_body` to record that the node originated from an
/// Anthropic `redacted_thinking` block, so that the Anthropic encoder can reconstruct the
/// original block type. See `spec/unified_responses_proxy.spec.md` PM5 / DM5.1.
pub const REASONING_KIND_EXTRA_KEY: &str = "_monoize_reasoning_kind";
pub const REASONING_KIND_REDACTED_THINKING: &str = "redacted_thinking";
pub const REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY: &str =
    "_monoize_reasoning_downstream_only_presentation";
/// Full OpenRouter `reasoning_details[]` entry retained on one reasoning node.
/// One source entry maps to one node so repeated entry types and entry order survive replay.
pub const CHAT_REASONING_DETAIL_EXTRA_KEY: &str = "_monoize_chat_reasoning_detail";
/// Scalar Chat reasoning surface that supplied a reasoning node when no structured detail existed.
pub const CHAT_REASONING_SURFACE_EXTRA_KEY: &str = "_monoize_chat_reasoning_surface";
pub const CHAT_REASONING_SURFACE_REASONING: &str = "reasoning";
pub const CHAT_REASONING_SURFACE_REASONING_CONTENT: &str = "reasoning_content";
/// Exact provider-specific request controls kept out of cross-family generic reasoning fields.
pub const CHAT_REASONING_CONFIG_EXTRA_KEY: &str = "_monoize_chat_reasoning_config";
pub const CHAT_THINKING_CONFIG_EXTRA_KEY: &str = "_monoize_chat_thinking_config";
pub const CHAT_MESSAGE_AUDIO_EXTRA_KEY: &str = "_monoize_chat_message_audio";
pub const CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY: &str =
    "_monoize_chat_legacy_function_definition";
pub const CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY: &str = "_monoize_chat_legacy_function_choice";
pub const CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY: &str = "_monoize_chat_legacy_function_call";
pub const CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY: &str = "_monoize_chat_legacy_function_result";
pub const MESSAGES_THINKING_CONFIG_EXTRA_KEY: &str = "_monoize_messages_thinking_config";
pub const MESSAGES_OUTPUT_CONFIG_EXTRA_KEY: &str = "_monoize_messages_output_config";
pub const FILE_ID_ORIGIN_EXTRA_KEY: &str = "_monoize_file_id_origin";
pub const FILE_ID_ORIGIN_OPENAI: &str = "openai";
pub const FILE_ID_ORIGIN_MESSAGES: &str = "messages";
/// Typed usage observed on an upstream Messages `message_start`, retained until the
/// downstream Messages encoder emits its own `message_start` envelope.
pub const MESSAGES_STREAM_START_USAGE_EXTRA_KEY: &str = "_monoize_messages_stream_start_usage";
pub const RESPONSES_REASONING_SUMMARY_EXTRA_KEY: &str = "_monoize_responses_reasoning_summary";
pub const RESPONSES_REASONING_CONTENT_EXTRA_KEY: &str = "_monoize_responses_reasoning_content";
/// Complete native Responses `image_generation_call` item retained on a semantic Image node.
pub const RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY: &str =
    "_monoize_responses_image_generation_call";
/// Exact top-level Responses `instructions` value retained for same-protocol request replay.
pub const RESPONSES_INSTRUCTIONS_EXTRA_KEY: &str = "_monoize_responses_instructions";
/// Marks semantic nodes decoded from top-level Responses `instructions`.
pub const RESPONSES_INSTRUCTION_NODE_EXTRA_KEY: &str = "_monoize_responses_instruction_node";
/// Complete non-stream Responses object retained so absent optional fields remain absent.
pub const RESPONSES_RESPONSE_SOURCE_EXTRA_KEY: &str = "_monoize_responses_response_source";
/// Upstream Responses start object retained for same-protocol stream envelope reconstruction.
pub const RESPONSES_STREAM_START_SOURCE_EXTRA_KEY: &str = "_monoize_responses_stream_start_source";
pub const REASONING_ENVELOPE_PREFIX: &str = "mz2.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningEnvelope {
    pub v: u8,
    pub provider_type: String,
    pub model: String,
    pub item_id: Option<String>,
    pub payload: Value,
}

/// Wrap `(item_id, signature)` into a sigil string suitable for smuggling through a downstream
/// Anthropic `thinking.signature` or `redacted_thinking.data` field. Returns `None` when either
/// input is empty.
pub fn wrap_reasoning_signature_with_item_id(item_id: &str, signature: &str) -> Option<String> {
    if item_id.is_empty() || signature.is_empty() {
        return None;
    }
    if is_reasoning_signature_sigil(signature) {
        return Some(signature.to_string());
    }
    Some(format!(
        "{REASONING_SIGNATURE_SIGIL_PREFIX}{item_id}.{signature}"
    ))
}

pub fn is_reasoning_signature_sigil(signature: &str) -> bool {
    signature.starts_with(REASONING_SIGNATURE_SIGIL_PREFIX)
}

/// Parse a sigil-encoded signature string into `(item_id, original_signature)`. Returns `None`
/// when the input is not sigil-encoded or is malformed. The item id segment is the substring
/// between the prefix and the first `.`; everything after that `.` is the original signature
/// returned verbatim.
pub fn unwrap_reasoning_signature_sigil(signature: &str) -> Option<(String, String)> {
    let rest = signature.strip_prefix(REASONING_SIGNATURE_SIGIL_PREFIX)?;
    let dot = rest.find('.')?;
    let id = &rest[..dot];
    let original = &rest[dot + 1..];
    if id.is_empty() || original.is_empty() {
        return None;
    }
    Some((id.to_string(), original.to_string()))
}

/// If `signature` is sigil-encoded, return the original signature. Otherwise return it unchanged.
pub fn strip_reasoning_signature_sigil(signature: &str) -> String {
    unwrap_reasoning_signature_sigil(signature)
        .map(|(_, original)| original)
        .unwrap_or_else(|| signature.to_string())
}

pub fn parse_reasoning_envelope(value: &Value) -> Option<ReasoningEnvelope> {
    let raw = value.as_str()?;
    if let Some(encoded) = raw.strip_prefix(REASONING_ENVELOPE_PREFIX) {
        let decoded = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).ok()?;
        let envelope = serde_json::from_slice::<ReasoningEnvelope>(&decoded).ok()?;
        if envelope.v == 2 && !envelope.provider_type.is_empty() && !envelope.model.is_empty() {
            return Some(envelope);
        }
        return None;
    }

    let (item_id, payload) = unwrap_reasoning_signature_sigil(raw)?;
    Some(ReasoningEnvelope {
        v: 1,
        provider_type: String::new(),
        model: String::new(),
        item_id: Some(item_id),
        payload: Value::String(payload),
    })
}

fn reasoning_envelope_matches(
    envelope: &ReasoningEnvelope,
    provider_type: &str,
    model: &str,
) -> bool {
    envelope.v == 1 || (envelope.provider_type == provider_type && envelope.model == model)
}

fn wrap_reasoning_payload(
    encrypted: &mut Option<Value>,
    item_id: Option<&str>,
    provider_type: &str,
    model: &str,
) {
    let Some(payload) = encrypted.take() else {
        return;
    };
    if parse_reasoning_envelope(&payload).is_some() {
        *encrypted = Some(payload);
        return;
    }

    let envelope = ReasoningEnvelope {
        v: 2,
        provider_type: provider_type.to_string(),
        model: model.to_string(),
        item_id: item_id.filter(|id| !id.is_empty()).map(str::to_string),
        payload,
    };
    let Ok(bytes) = serde_json::to_vec(&envelope) else {
        *encrypted = Some(envelope.payload);
        return;
    };
    *encrypted = Some(Value::String(format!(
        "{REASONING_ENVELOPE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    )));
}

fn wrap_reasoning_extra_body_encrypted_content(
    extra_body: &mut HashMap<String, Value>,
    item_id: Option<&str>,
    provider_type: &str,
    model: &str,
) {
    let Some(value) = extra_body.remove("encrypted_content") else {
        return;
    };
    let mut encrypted = Some(value);
    wrap_reasoning_payload(&mut encrypted, item_id, provider_type, model);
    if let Some(value) = encrypted {
        extra_body.insert("encrypted_content".to_string(), value);
    }
}

fn extra_body_is_reasoning_item(extra_body: &HashMap<String, Value>) -> bool {
    extra_body.contains_key("encrypted_content")
        || extra_body.get("type").and_then(Value::as_str) == Some("reasoning")
}

fn wrap_reasoning_node_envelope(node: &mut Node, provider_type: &str, model: &str) {
    if let Node::Reasoning {
        id,
        encrypted,
        extra_body,
        ..
    } = node
    {
        wrap_reasoning_payload(encrypted, id.as_deref(), provider_type, model);
        wrap_reasoning_extra_body_encrypted_content(
            extra_body,
            id.as_deref(),
            provider_type,
            model,
        );
    }
}

pub fn wrap_reasoning_envelopes_in_response(
    response: &mut UrpResponse,
    provider_type: &str,
    model: &str,
) {
    for node in &mut response.output {
        wrap_reasoning_node_envelope(node, provider_type, model);
    }
}

pub fn wrap_reasoning_envelope_in_stream_event(
    event: &mut UrpStreamEvent,
    provider_type: &str,
    model: &str,
) {
    match event {
        UrpStreamEvent::NodeStart {
            header: NodeHeader::Reasoning { id },
            extra_body,
            ..
        } => wrap_reasoning_extra_body_encrypted_content(
            extra_body,
            id.as_deref(),
            provider_type,
            model,
        ),
        UrpStreamEvent::NodeStart {
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            extra_body,
            ..
        } if extra_body_is_reasoning_item(extra_body) => {
            let item_id = extra_body
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| extra_body.get("reasoning_item_id").and_then(Value::as_str))
                .or_else(|| extra_body.get("item_id").and_then(Value::as_str))
                .map(str::to_string);
            wrap_reasoning_extra_body_encrypted_content(
                extra_body,
                item_id.as_deref(),
                provider_type,
                model,
            );
        }
        UrpStreamEvent::NodeDelta {
            delta: NodeDelta::Reasoning { encrypted, .. },
            extra_body,
            ..
        } => {
            let item_id = extra_body
                .get("reasoning_item_id")
                .and_then(Value::as_str)
                .or_else(|| extra_body.get("item_id").and_then(Value::as_str));
            wrap_reasoning_payload(encrypted, item_id, provider_type, model);
        }
        UrpStreamEvent::NodeDone { node, .. } => {
            wrap_reasoning_node_envelope(node, provider_type, model);
            if let Node::NextDownstreamEnvelopeExtra { extra_body } = node
                && extra_body_is_reasoning_item(extra_body)
            {
                let item_id = extra_body
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                wrap_reasoning_extra_body_encrypted_content(
                    extra_body,
                    item_id.as_deref(),
                    provider_type,
                    model,
                );
            }
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                wrap_reasoning_node_envelope(node, provider_type, model);
            }
        }
        _ => {}
    }
}

pub fn filter_and_unwrap_reasoning_envelopes_for_upstream(
    nodes: &mut Vec<Node>,
    provider_type: &str,
    model: &str,
    enforce_match: bool,
) {
    nodes.retain_mut(|node| {
        let Node::Reasoning {
            id,
            encrypted,
            extra_body,
            ..
        } = node
        else {
            return true;
        };
        if let Some(envelope) = encrypted.as_ref().and_then(parse_reasoning_envelope) {
            if enforce_match && !reasoning_envelope_matches(&envelope, provider_type, model) {
                return false;
            }
            if let Some(envelope_id) = envelope.item_id {
                if !envelope_id.is_empty() {
                    *id = Some(envelope_id);
                }
            }
            *encrypted = Some(envelope.payload);
        }
        if let Some(envelope) = extra_body
            .get("encrypted_content")
            .and_then(parse_reasoning_envelope)
        {
            if enforce_match && !reasoning_envelope_matches(&envelope, provider_type, model) {
                return false;
            }
            if let Some(envelope_id) = envelope.item_id {
                if !envelope_id.is_empty() {
                    *id = Some(envelope_id);
                }
            }
            extra_body.insert("encrypted_content".to_string(), envelope.payload);
        }
        true
    });
}

pub fn synthetic_tool_result_id() -> String {
    format!("fco_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_provider_item_id() -> String {
    format!("pi_urp_{}", uuid::Uuid::new_v4().simple())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    OpenaiImage,
    Replicate,
}

impl ProviderProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderProtocol::Responses => "responses",
            ProviderProtocol::ChatCompletion => "chat_completion",
            ProviderProtocol::Messages => "messages",
            ProviderProtocol::Gemini => "gemini",
            ProviderProtocol::OpenaiImage => "openai_image",
            ProviderProtocol::Replicate => "replicate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopControl {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrpRequest {
    pub model: String,
    #[serde(alias = "inputs")]
    pub input: Vec<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Text {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        source: ImageSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Audio {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        source: AudioSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        source: FileSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Refusal {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        content: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
        name: String,
        arguments: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        origin_protocol: ProviderProtocol,
        role: OrdinaryRole,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
        #[serde(default)]
        is_error: bool,
        content: Vec<ToolResultContent>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NextDownstreamEnvelopeExtra {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrdinaryRole {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallType {
    #[default]
    Function,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Url {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Base64 {
        media_type: String,
        data: String,
    },
    FileId {
        file_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioSource {
    Url { url: String },
    Base64 { media_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSource {
    Url {
        url: String,
    },
    FileId {
        file_id: String,
    },
    Text {
        text: String,
    },
    Content {
        content: Vec<Value>,
    },
    Base64 {
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        media_type: String,
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text {
        text: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Image {
        source: ImageSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        source: FileSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        origin_protocol: ProviderProtocol,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

impl ToolResultContent {
    pub fn extra_body_mut(&mut self) -> &mut HashMap<String, Value> {
        match self {
            Self::Text { extra_body, .. }
            | Self::Image { extra_body, .. }
            | Self::File { extra_body, .. }
            | Self::ProviderItem { extra_body, .. } => extra_body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomToolDefinition>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Value>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Mode(String),
    Specific(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: JsonSchemaDefinition },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchemaDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrpResponse {
    pub id: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(alias = "outputs")]
    pub output: Vec<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModalityBreakdown {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputDetails {
    #[serde(default)]
    pub standard_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_modality_breakdown: Option<ModalityBreakdown>,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_creation_5m_tokens: u64,
    #[serde(default)]
    pub cache_creation_1h_tokens: u64,
    #[serde(default)]
    pub tool_prompt_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modality_breakdown: Option<ModalityBreakdown>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputDetails {
    #[serde(default)]
    pub standard_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub accepted_prediction_tokens: u64,
    #[serde(default)]
    pub rejected_prediction_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modality_breakdown: Option<ModalityBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_details: Option<InputDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_details: Option<OutputDetails>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn cached_tokens(&self) -> Option<u64> {
        self.input_details
            .as_ref()
            .map(|d| d.cache_read_tokens)
            .filter(|&v| v > 0)
    }

    pub fn reasoning_tokens(&self) -> Option<u64> {
        self.output_details
            .as_ref()
            .map(|d| d.reasoning_tokens)
            .filter(|&v| v > 0)
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UrpStreamEvent {
    ResponseStart {
        id: String,
        model: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NodeStart {
        node_index: u32,
        header: NodeHeader,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NodeDelta {
        node_index: u32,
        delta: NodeDelta,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NodeDone {
        node_index: u32,
        node: Node,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ResponseDone {
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(alias = "outputs")]
        output: Vec<Node>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderControl {
        protocol: String,
        event_name: String,
        data: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Error {
        code: Option<String>,
        message: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeHeader {
    Text {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
    },
    Audio {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
    },
    File {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
    },
    Refusal {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
        name: String,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        origin_protocol: ProviderProtocol,
        role: OrdinaryRole,
        item_type: String,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
    },
    NextDownstreamEnvelopeExtra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeDelta {
    Text {
        content: String,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    Refusal {
        content: String,
    },
    ToolCallArguments {
        arguments: String,
    },
    Image {
        source: ImageSource,
    },
    Audio {
        source: AudioSource,
    },
    File {
        source: FileSource,
    },
    ProviderItem {
        data: Value,
    },
}

impl Node {
    pub fn text(role: OrdinaryRole, content: impl Into<String>) -> Self {
        Node::Text {
            id: None,
            role,
            content: content.into(),
            phase: None,
            extra_body: HashMap::new(),
        }
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::text(OrdinaryRole::Assistant, content)
    }

    pub fn role(&self) -> Option<OrdinaryRole> {
        match self {
            Node::Text { role, .. }
            | Node::Image { role, .. }
            | Node::Audio { role, .. }
            | Node::File { role, .. }
            | Node::ProviderItem { role, .. } => Some(*role),
            Node::Refusal { .. } | Node::Reasoning { .. } | Node::ToolCall { .. } => {
                Some(OrdinaryRole::Assistant)
            }
            Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    pub fn extra_body_mut(&mut self) -> &mut HashMap<String, Value> {
        match self {
            Node::Text { extra_body, .. }
            | Node::Image { extra_body, .. }
            | Node::Audio { extra_body, .. }
            | Node::File { extra_body, .. }
            | Node::Refusal { extra_body, .. }
            | Node::Reasoning { extra_body, .. }
            | Node::ToolCall { extra_body, .. }
            | Node::ProviderItem { extra_body, .. }
            | Node::ToolResult { extra_body, .. }
            | Node::NextDownstreamEnvelopeExtra { extra_body, .. } => extra_body,
        }
    }
    pub fn id(&self) -> Option<&String> {
        match self {
            Node::Text { id, .. }
            | Node::Image { id, .. }
            | Node::Audio { id, .. }
            | Node::File { id, .. }
            | Node::Refusal { id, .. }
            | Node::Reasoning { id, .. }
            | Node::ToolCall { id, .. }
            | Node::ProviderItem { id, .. }
            | Node::ToolResult { id, .. } => id.as_ref(),
            Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    pub fn set_id(&mut self, new_id: Option<String>) {
        match self {
            Node::Text { id, .. }
            | Node::Image { id, .. }
            | Node::Audio { id, .. }
            | Node::File { id, .. }
            | Node::Refusal { id, .. }
            | Node::Reasoning { id, .. }
            | Node::ToolCall { id, .. }
            | Node::ProviderItem { id, .. }
            | Node::ToolResult { id, .. } => *id = new_id,
            Node::NextDownstreamEnvelopeExtra { .. } => {}
        }
    }
}

pub fn node_is_empty_text(node: &Node) -> bool {
    matches!(node, Node::Text { content, .. } if content.is_empty())
}

pub fn nodes_semantically_match(left: &Node, right: &Node) -> bool {
    match (left, right) {
        (
            Node::Text {
                id: left_id,
                role: left_role,
                phase: left_phase,
                content: left_content,
                ..
            },
            Node::Text {
                id: right_id,
                role: right_role,
                phase: right_phase,
                content: right_content,
                ..
            },
        ) => {
            let phases_compatible =
                left_phase == right_phase || left_phase.is_none() || right_phase.is_none();
            (left_id.is_some() && left_id == right_id)
                || (left_role == right_role && phases_compatible && left_content == right_content)
        }
        (
            Node::Image {
                id: left_id,
                source: left_source,
                ..
            },
            Node::Image {
                id: right_id,
                source: right_source,
                ..
            },
        ) => (left_id.is_some() && left_id == right_id) || left_source == right_source,
        (Node::Audio { id: left_id, .. }, Node::Audio { id: right_id, .. })
        | (Node::File { id: left_id, .. }, Node::File { id: right_id, .. })
        | (Node::Refusal { id: left_id, .. }, Node::Refusal { id: right_id, .. }) => {
            left_id.is_some() && left_id == right_id
        }
        (
            Node::ProviderItem {
                id: left_id,
                origin_protocol: left_origin,
                ..
            },
            Node::ProviderItem {
                id: right_id,
                origin_protocol: right_origin,
                ..
            },
        ) => left_origin == right_origin && left_id.is_some() && left_id == right_id,
        (
            Node::Reasoning {
                id: left_id,
                content: left_content,
                encrypted: left_encrypted,
                summary: left_summary,
                source: left_source,
                extra_body: left_extra_body,
            },
            Node::Reasoning {
                id: right_id,
                content: right_content,
                encrypted: right_encrypted,
                summary: right_summary,
                source: right_source,
                extra_body: right_extra_body,
            },
        ) => {
            (left_id.is_some() && left_id == right_id)
                || (left_content == right_content
                    && left_encrypted == right_encrypted
                    && left_summary == right_summary
                    && left_source == right_source
                    && left_extra_body == right_extra_body)
        }
        (
            Node::ToolCall {
                id: left_id,
                call_id: left_call_id,
                ..
            },
            Node::ToolCall {
                id: right_id,
                call_id: right_call_id,
                ..
            },
        )
        | (
            Node::ToolResult {
                id: left_id,
                call_id: left_call_id,
                ..
            },
            Node::ToolResult {
                id: right_id,
                call_id: right_call_id,
                ..
            },
        ) => left_call_id == right_call_id || (left_id.is_some() && left_id == right_id),
        _ => left == right,
    }
}

pub fn push_unique_node(output: &mut Vec<Node>, node: Node) {
    if node_is_empty_text(&node) {
        return;
    }
    if !output
        .iter()
        .any(|candidate| nodes_semantically_match(candidate, &node))
    {
        output.push(node);
    }
}

pub fn strip_nested_extra_body(nodes: &mut Vec<Node>) {
    fn retain_internal_metadata(extra_body: &mut HashMap<String, Value>) {
        extra_body.retain(|key, _| key.starts_with("_monoize_"));
    }

    for node in nodes.iter_mut() {
        match node {
            Node::Text { extra_body, .. }
            | Node::Image { extra_body, .. }
            | Node::Audio { extra_body, .. }
            | Node::File { extra_body, .. }
            | Node::Refusal { extra_body, .. }
            | Node::Reasoning { extra_body, .. }
            | Node::ToolCall { extra_body, .. }
            | Node::ProviderItem { extra_body, .. } => {
                retain_internal_metadata(extra_body);
            }
            Node::ToolResult {
                content,
                extra_body,
                ..
            } => {
                retain_internal_metadata(extra_body);
                for item in content {
                    retain_internal_metadata(item.extra_body_mut());
                }
            }
            Node::NextDownstreamEnvelopeExtra { .. } => {}
        }
    }
    nodes.retain(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }));
}

pub fn retain_provider_items_for_protocol(nodes: &mut Vec<Node>, target: ProviderProtocol) {
    for node in nodes.iter_mut() {
        if let Node::ToolResult { content, .. } = node {
            content.retain(|item| {
                !matches!(
                    item,
                    ToolResultContent::ProviderItem {
                        origin_protocol,
                        ..
                    } if *origin_protocol != target
                )
            });
        }
    }
    nodes.retain(|node| {
        !matches!(
            node,
            Node::ProviderItem {
                origin_protocol,
                ..
            } if *origin_protocol != target
        )
    });
}

pub fn remove_downstream_only_reasoning_for_responses(nodes: &mut Vec<Node>) {
    nodes.retain(|node| {
        !matches!(
            node,
            Node::Reasoning { extra_body, .. }
                if extra_body
                    .get(REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY)
                    .and_then(Value::as_bool)
                    == Some(true)
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_role_returns_explicit_role_for_ordinary_nodes() {
        let text = Node::text(OrdinaryRole::User, "hello");
        assert_eq!(text.role(), Some(OrdinaryRole::User));

        let img = Node::Image {
            id: None,
            role: OrdinaryRole::Assistant,
            source: ImageSource::Url {
                url: "http://x".into(),
                detail: None,
            },
            extra_body: HashMap::new(),
        };
        assert_eq!(img.role(), Some(OrdinaryRole::Assistant));
    }

    #[test]
    fn node_role_returns_assistant_for_implicit_assistant_nodes() {
        let refusal = Node::Refusal {
            id: None,
            content: "no".into(),
            extra_body: HashMap::new(),
        };
        assert_eq!(refusal.role(), Some(OrdinaryRole::Assistant));

        let reasoning = Node::Reasoning {
            id: None,
            content: Some("think".into()),
            encrypted: None,
            summary: None,
            source: None,
            extra_body: HashMap::new(),
        };
        assert_eq!(reasoning.role(), Some(OrdinaryRole::Assistant));

        let tc = Node::ToolCall {
            id: None,
            tool_type: ToolCallType::Function,
            call_id: "c1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            extra_body: HashMap::new(),
        };
        assert_eq!(tc.role(), Some(OrdinaryRole::Assistant));
    }

    #[test]
    fn node_role_returns_none_for_tool_result_and_control() {
        let tr = Node::ToolResult {
            id: None,
            tool_type: ToolCallType::Function,
            call_id: "c1".into(),
            is_error: false,
            content: vec![],
            extra_body: HashMap::new(),
        };
        assert_eq!(tr.role(), None);

        let ctrl = Node::NextDownstreamEnvelopeExtra {
            extra_body: HashMap::new(),
        };
        assert_eq!(ctrl.role(), None);
    }

    #[test]
    fn tool_result_stays_distinct_in_flat_vec() {
        let nodes: Vec<Node> = vec![
            Node::text(OrdinaryRole::User, "hi"),
            Node::ToolResult {
                id: None,
                tool_type: ToolCallType::Function,
                call_id: "c1".into(),
                is_error: false,
                content: vec![ToolResultContent::Text {
                    text: "ok".into(),
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            },
            Node::assistant_text("reply"),
        ];
        assert!(matches!(nodes[1], Node::ToolResult { .. }));
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn strip_nested_extra_body_keeps_internal_metadata_and_removes_wire_extras() {
        let mut nodes = vec![
            Node::Text {
                id: None,
                role: OrdinaryRole::Assistant,
                content: "hi".into(),
                phase: Some("commentary".into()),
                extra_body: [
                    ("x".into(), serde_json::json!(1)),
                    ("_monoize_text_semantics".into(), serde_json::json!(4)),
                ]
                .into_iter()
                .collect(),
            },
            Node::NextDownstreamEnvelopeExtra {
                extra_body: [("y".into(), serde_json::json!(2))].into_iter().collect(),
            },
            Node::ToolResult {
                id: None,
                tool_type: ToolCallType::Function,
                call_id: "call_1".into(),
                is_error: false,
                content: vec![ToolResultContent::Text {
                    text: "ok".into(),
                    extra_body: [
                        ("nested".into(), serde_json::json!(true)),
                        ("_monoize_content_semantics".into(), serde_json::json!(5)),
                    ]
                    .into_iter()
                    .collect(),
                }],
                extra_body: [
                    ("z".into(), serde_json::json!(3)),
                    ("_monoize_result_semantics".into(), serde_json::json!(6)),
                ]
                .into_iter()
                .collect(),
            },
        ];
        strip_nested_extra_body(&mut nodes);
        assert_eq!(nodes.len(), 2);
        assert!(matches!(&nodes[0], Node::Text { extra_body, .. }
        if extra_body == &HashMap::from([(
            "_monoize_text_semantics".to_string(),
            serde_json::json!(4),
        )])));
        assert!(
            matches!(&nodes[1], Node::ToolResult { id: _, extra_body, .. }
            if extra_body == &HashMap::from([(
                "_monoize_result_semantics".to_string(),
                serde_json::json!(6),
            )]))
        );
        assert!(matches!(
            &nodes[1],
            Node::ToolResult { content, .. }
                if matches!(&content[0], ToolResultContent::Text { extra_body, .. }
                    if extra_body == &HashMap::from([(
                        "_monoize_content_semantics".to_string(),
                        serde_json::json!(5),
                    )]))
        ));
    }

    #[test]
    fn responses_prestrip_removes_only_downstream_only_reasoning() {
        let mut nodes = vec![
            Node::Reasoning {
                id: None,
                content: None,
                encrypted: None,
                summary: Some("messages summary".into()),
                source: None,
                extra_body: [
                    (
                        REASONING_DOWNSTREAM_ONLY_PRESENTATION_EXTRA_KEY.into(),
                        serde_json::json!(true),
                    ),
                    ("nested".into(), serde_json::json!(1)),
                ]
                .into_iter()
                .collect(),
            },
            Node::Reasoning {
                id: Some("rs_raw".into()),
                content: Some("raw reasoning".into()),
                encrypted: None,
                summary: None,
                source: None,
                extra_body: [("nested".into(), serde_json::json!(2))]
                    .into_iter()
                    .collect(),
            },
        ];

        remove_downstream_only_reasoning_for_responses(&mut nodes);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(
            &nodes[0],
            Node::Reasoning {
                id: Some(id),
                content: Some(content),
                ..
            } if id == "rs_raw" && content == "raw reasoning"
        ));

        strip_nested_extra_body(&mut nodes);
        assert!(matches!(
            &nodes[0],
            Node::Reasoning {
                content: Some(content),
                extra_body,
                ..
            } if content == "raw reasoning" && extra_body.is_empty()
        ));
    }

    #[test]
    fn retain_provider_items_filters_nested_tool_result_content_by_exact_protocol() {
        let mut nodes = vec![Node::ToolResult {
            id: None,
            tool_type: ToolCallType::Function,
            call_id: "call_1".into(),
            is_error: false,
            content: vec![
                ToolResultContent::Text {
                    text: "ok".into(),
                    extra_body: HashMap::new(),
                },
                ToolResultContent::ProviderItem {
                    origin_protocol: ProviderProtocol::Messages,
                    item_type: "search_result".into(),
                    body: serde_json::json!({ "type": "search_result" }),
                    extra_body: HashMap::new(),
                },
                ToolResultContent::ProviderItem {
                    origin_protocol: ProviderProtocol::Responses,
                    item_type: "computer_screenshot".into(),
                    body: serde_json::json!({ "type": "computer_screenshot" }),
                    extra_body: HashMap::new(),
                },
            ],
            extra_body: HashMap::new(),
        }];

        retain_provider_items_for_protocol(&mut nodes, ProviderProtocol::Messages);
        let Node::ToolResult { content, .. } = &nodes[0] else {
            panic!("expected tool result");
        };
        assert_eq!(content.len(), 2);
        assert!(matches!(
            content[1],
            ToolResultContent::ProviderItem {
                origin_protocol: ProviderProtocol::Messages,
                ..
            }
        ));
    }

    #[test]
    fn control_node_is_filtered_by_strip_nested_extra_body_on_nodes() {
        let mut nodes = vec![
            Node::text(OrdinaryRole::User, "hi"),
            Node::NextDownstreamEnvelopeExtra {
                extra_body: [("k".into(), serde_json::json!("v"))].into_iter().collect(),
            },
            Node::assistant_text("reply"),
        ];
        // Verify node vec preserves control node before stripping
        assert_eq!(nodes.len(), 3);
        assert!(matches!(
            &nodes[1],
            Node::NextDownstreamEnvelopeExtra { .. }
        ));

        strip_nested_extra_body(&mut nodes);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn streaming_reasoning_delta_envelope_uses_reasoning_item_id_extra() {
        let mut event = UrpStreamEvent::NodeDelta {
            node_index: 0,
            delta: NodeDelta::Reasoning {
                content: None,
                encrypted: Some(serde_json::json!("opaque_payload")),
                summary: None,
                source: None,
            },
            usage: None,
            extra_body: [(
                "reasoning_item_id".to_string(),
                serde_json::json!("rs_original"),
            )]
            .into_iter()
            .collect(),
        };

        wrap_reasoning_envelope_in_stream_event(&mut event, "responses", "gpt-5.5");
        let UrpStreamEvent::NodeDelta {
            delta:
                NodeDelta::Reasoning {
                    encrypted: Some(encrypted),
                    ..
                },
            ..
        } = event
        else {
            panic!("expected reasoning delta");
        };
        let envelope = parse_reasoning_envelope(&encrypted).expect("mz2 envelope");
        assert_eq!(envelope.item_id.as_deref(), Some("rs_original"));
        assert_eq!(envelope.payload, serde_json::json!("opaque_payload"));
    }

    #[test]
    fn streaming_reasoning_envelope_extra_uses_item_id() {
        let mut event = UrpStreamEvent::NodeStart {
            node_index: 0,
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            extra_body: [
                ("id".to_string(), serde_json::json!("rs_original")),
                (
                    "encrypted_content".to_string(),
                    serde_json::json!("opaque_payload"),
                ),
            ]
            .into_iter()
            .collect(),
        };

        wrap_reasoning_envelope_in_stream_event(&mut event, "responses", "gpt-5.5");
        let UrpStreamEvent::NodeStart { extra_body, .. } = event else {
            panic!("expected envelope extra start");
        };
        let encrypted = extra_body
            .get("encrypted_content")
            .expect("encrypted content");
        let envelope = parse_reasoning_envelope(encrypted).expect("mz2 envelope");
        assert_eq!(envelope.item_id.as_deref(), Some("rs_original"));
        assert_eq!(envelope.payload, serde_json::json!("opaque_payload"));
    }

    #[test]
    fn node_greedy_merger_flushes_on_role_change() {
        use crate::urp::greedy::{NodeAction, NodeGreedyMerger};
        let mut m = NodeGreedyMerger::new();
        assert!(matches!(
            m.feed(Node::text(OrdinaryRole::User, "a")),
            NodeAction::Append
        ));
        match m.feed(Node::text(OrdinaryRole::Assistant, "b")) {
            NodeAction::FlushAndNew(flushed) => {
                assert_eq!(flushed.len(), 1);
                assert!(matches!(
                    &flushed[0],
                    Node::Text {
                        role: OrdinaryRole::User,
                        ..
                    }
                ));
            }
            _ => panic!("expected flush"),
        }
        let rest = m.finish().unwrap();
        assert_eq!(rest.len(), 1);
    }
}

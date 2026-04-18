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

pub fn synthetic_tool_result_id() -> String {
    format!("fco_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_provider_item_id() -> String {
    format!("pi_urp_{}", uuid::Uuid::new_v4().simple())
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
        call_id: String,
        name: String,
        arguments: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
    Base64 {
        filename: Option<String>,
        media_type: String,
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
    Image { source: ImageSource },
    File { source: FileSource },
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionDefinition>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(flatten)]
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
    #[serde(default)]
    pub cache_creation_tokens: u64,
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
        call_id: String,
        name: String,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        item_type: String,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
            Node::Refusal { .. }
            | Node::Reasoning { .. }
            | Node::ToolCall { .. } => Some(OrdinaryRole::Assistant),
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

pub fn strip_nested_extra_body(nodes: &mut Vec<Node>) {
    for node in nodes.iter_mut() {
        match node {
            Node::Text { extra_body, .. }
            | Node::Image { extra_body, .. }
            | Node::Audio { extra_body, .. }
            | Node::File { extra_body, .. }
            | Node::Refusal { extra_body, .. }
            | Node::Reasoning { extra_body, .. }
            | Node::ToolCall { extra_body, .. }
            | Node::ProviderItem { extra_body, .. }
            | Node::ToolResult { extra_body, .. } => extra_body.clear(),
            Node::NextDownstreamEnvelopeExtra { .. } => {}
        }
    }
    nodes.retain(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }));
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
            source: ImageSource::Url { url: "http://x".into(), detail: None },
            extra_body: HashMap::new(),
        };
        assert_eq!(img.role(), Some(OrdinaryRole::Assistant));
    }

    #[test]
    fn node_role_returns_assistant_for_implicit_assistant_nodes() {
        let refusal = Node::Refusal { id: None, content: "no".into(), extra_body: HashMap::new() };
        assert_eq!(refusal.role(), Some(OrdinaryRole::Assistant));

        let reasoning = Node::Reasoning {
            id: None,
            content: Some("think".into()),
            encrypted: None, summary: None, source: None,
            extra_body: HashMap::new(),
        };
        assert_eq!(reasoning.role(), Some(OrdinaryRole::Assistant));

        let tc = Node::ToolCall {
            id: None,
            call_id: "c1".into(), name: "fn".into(), arguments: "{}".into(),
            extra_body: HashMap::new(),
        };
        assert_eq!(tc.role(), Some(OrdinaryRole::Assistant));
    }

    #[test]
    fn node_role_returns_none_for_tool_result_and_control() {
        let tr = Node::ToolResult {
            id: None,
            call_id: "c1".into(), is_error: false,
            content: vec![], extra_body: HashMap::new(),
        };
        assert_eq!(tr.role(), None);

        let ctrl = Node::NextDownstreamEnvelopeExtra { extra_body: HashMap::new() };
        assert_eq!(ctrl.role(), None);
    }

    #[test]
    fn tool_result_stays_distinct_in_flat_vec() {
        let nodes: Vec<Node> = vec![
            Node::text(OrdinaryRole::User, "hi"),
            Node::ToolResult {
                id: None,
                call_id: "c1".into(), is_error: false,
                content: vec![ToolResultContent::Text { text: "ok".into() }],
                extra_body: HashMap::new(),
            },
            Node::assistant_text("reply"),
        ];
        assert!(matches!(nodes[1], Node::ToolResult { .. }));
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn strip_nested_extra_body_clears_node_extras_and_control_nodes() {
        let mut nodes = vec![
            Node::Text {
                id: None,
                role: OrdinaryRole::Assistant,
                content: "hi".into(),
                phase: Some("commentary".into()),
                extra_body: [("x".into(), serde_json::json!(1))].into_iter().collect(),
            },
            Node::NextDownstreamEnvelopeExtra {
                extra_body: [("y".into(), serde_json::json!(2))].into_iter().collect(),
            },
            Node::ToolResult {
                id: None,
                call_id: "call_1".into(),
                is_error: false,
                content: vec![ToolResultContent::Text { text: "ok".into() }],
                extra_body: [("z".into(), serde_json::json!(3))].into_iter().collect(),
            },
        ];
        strip_nested_extra_body(&mut nodes);
        assert_eq!(nodes.len(), 2);
        assert!(matches!(&nodes[0], Node::Text { extra_body, .. } if extra_body.is_empty()));
        assert!(matches!(&nodes[1], Node::ToolResult { id: _, extra_body, .. } if extra_body.is_empty()));
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
        assert!(matches!(&nodes[1], Node::NextDownstreamEnvelopeExtra { .. }));

        strip_nested_extra_body(&mut nodes);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn node_greedy_merger_flushes_on_role_change() {
        use crate::urp::greedy::{NodeGreedyMerger, NodeAction};
        let mut m = NodeGreedyMerger::new();
        assert!(matches!(m.feed(Node::text(OrdinaryRole::User, "a")), NodeAction::Append));
        match m.feed(Node::text(OrdinaryRole::Assistant, "b")) {
            NodeAction::FlushAndNew(flushed) => {
                assert_eq!(flushed.len(), 1);
                assert!(matches!(&flushed[0], Node::Text { role: OrdinaryRole::User, .. }));
            }
            _ => panic!("expected flush"),
        }
        let rest = m.finish().unwrap();
        assert_eq!(rest.len(), 1);
    }
}

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub mod decode;
pub mod encode;
pub mod greedy;
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
    // Legacy grouped stream variants kept only for compile compatibility while
    // runtime stream semantics migrate to canonical flat node events.
    ItemStart {
        item_index: u32,
        header: ItemHeader,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    PartStart {
        part_index: u32,
        item_index: u32,
        header: PartHeader,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Delta {
        part_index: u32,
        delta: PartDelta,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    PartDone {
        part_index: u32,
        part: Part,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ItemDone {
        item_index: u32,
        item: Item,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
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
}

pub fn content_text(parts: &[Part]) -> String {
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Text { content, .. } | Part::Refusal { content, .. } => out.push_str(content),
            Part::Reasoning {
                content, summary, ..
            } => {
                if let Some(content) = content {
                    out.push_str(content);
                } else if let Some(summary) = summary {
                    out.push_str(summary);
                }
            }
            _ => {}
        }
    }
    out
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
    fn items_to_nodes_flattens_message() {
        let items = vec![
            Item::Message {
                id: None,
                role: Role::User,
                parts: vec![
                    Part::Text { content: "a".into(), extra_body: HashMap::new() },
                    Part::Text { content: "b".into(), extra_body: HashMap::new() },
                ],
                extra_body: HashMap::new(),
            },
            Item::ToolResult {
                id: None,
                call_id: "c1".into(), is_error: false,
                content: vec![ToolResultContent::Text { text: "r".into() }],
                extra_body: HashMap::new(),
            },
        ];
        let nodes = items_to_nodes(items);
        assert_eq!(nodes.len(), 3);
        assert!(matches!(&nodes[0], Node::Text { role: OrdinaryRole::User, content, .. } if content == "a"));
        assert!(matches!(&nodes[1], Node::Text { role: OrdinaryRole::User, content, .. } if content == "b"));
        assert!(matches!(&nodes[2], Node::ToolResult { call_id, .. } if call_id == "c1"));
    }

    #[test]
    fn nodes_to_items_groups_by_role() {
        let nodes = vec![
            Node::text(OrdinaryRole::User, "a"),
            Node::text(OrdinaryRole::User, "b"),
            Node::text(OrdinaryRole::Assistant, "c"),
        ];
        let items = nodes_to_items(&nodes);
        assert_eq!(items.len(), 2);
        if let Item::Message { id: _, role, parts, .. } = &items[0] {
            assert_eq!(*role, Role::User);
            assert_eq!(parts.len(), 2);
        } else {
            panic!("expected message");
        }
        if let Item::Message { id: _, role, parts, .. } = &items[1] {
            assert_eq!(*role, Role::Assistant);
            assert_eq!(parts.len(), 1);
        } else {
            panic!("expected message");
        }
    }

    #[test]
    fn nodes_to_items_preserves_tool_result_boundary() {
        let nodes = vec![
            Node::text(OrdinaryRole::User, "before"),
            Node::ToolResult {
                id: None,
                call_id: "c1".into(), is_error: false,
                content: vec![], extra_body: HashMap::new(),
            },
            Node::text(OrdinaryRole::Assistant, "after"),
        ];
        let items = nodes_to_items(&nodes);
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], Item::Message { role: Role::User, .. }));
        assert!(matches!(&items[1], Item::ToolResult { call_id, .. } if call_id == "c1"));
        assert!(matches!(&items[2], Item::Message { role: Role::Assistant, .. }));
    }

    #[test]
    fn nodes_to_items_keeps_reasoning_with_first_phased_text_and_splits_later_phase() {
        let nodes = vec![
            Node::Reasoning {
                id: None,
                content: Some("hmm".into()),
                encrypted: None,
                summary: None,
                source: None,
                extra_body: HashMap::new(),
            },
            Node::Text {
                id: None,
                role: OrdinaryRole::Assistant,
                content: "phase A".into(),
                phase: Some("commentary".into()),
                extra_body: HashMap::new(),
            },
            Node::Text {
                id: None,
                role: OrdinaryRole::Assistant,
                content: "phase B".into(),
                phase: Some("final_answer".into()),
                extra_body: HashMap::new(),
            },
            Node::ToolCall {
                id: None,
                call_id: "call_2".into(),
                name: "tool_b".into(),
                arguments: "{}".into(),
                extra_body: HashMap::new(),
            },
        ];

        let items = nodes_to_items(&nodes);
        assert_eq!(items.len(), 2);

        match &items[0] {
            Item::Message { id: _, role, parts, extra_body } => {
                assert_eq!(*role, Role::Assistant);
                assert_eq!(parts.len(), 2);
                assert!(matches!(&parts[0], Part::Reasoning { content: Some(text), .. } if text == "hmm"));
                assert!(matches!(&parts[1], Part::Text { content, .. } if content == "phase A"));
                assert_eq!(extra_body.get("phase"), Some(&serde_json::json!("commentary")));
            }
            _ => panic!("expected assistant message"),
        }

        match &items[1] {
            Item::Message { id: _, role, parts, extra_body } => {
                assert_eq!(*role, Role::Assistant);
                assert_eq!(parts.len(), 2);
                assert!(matches!(&parts[0], Part::Text { content, .. } if content == "phase B"));
                assert!(matches!(&parts[1], Part::ToolCall { call_id, .. } if call_id == "call_2"));
                assert_eq!(extra_body.get("phase"), Some(&serde_json::json!("final_answer")));
            }
            _ => panic!("expected assistant message"),
        }
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
        let nodes = vec![
            Node::text(OrdinaryRole::User, "hi"),
            Node::NextDownstreamEnvelopeExtra {
                extra_body: [("k".into(), serde_json::json!("v"))].into_iter().collect(),
            },
            Node::assistant_text("reply"),
        ];
        let items_before = nodes_to_items(&nodes);
        // Control node should not appear in items
        assert_eq!(items_before.len(), 2);

        // Verify node vec preserves control node
        assert_eq!(nodes.len(), 3);
        assert!(matches!(&nodes[1], Node::NextDownstreamEnvelopeExtra { .. }));
    }

    #[test]
    fn items_to_nodes_with_envelope_control_round_trips_message_extra_once() {
        let items = vec![
            Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Text {
                    content: "first".into(),
                    extra_body: HashMap::new(),
                }],
                extra_body: [("first_only".into(), serde_json::json!("A"))]
                    .into_iter()
                    .collect(),
            },
            Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Text {
                    content: "second".into(),
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            },
        ];

        let nodes = items_to_nodes_with_envelope_control(items);
        assert!(matches!(
            &nodes[0],
            Node::NextDownstreamEnvelopeExtra { extra_body }
            if extra_body.get("first_only") == Some(&serde_json::json!("A"))
        ));
        assert!(matches!(&nodes[1], Node::Text { content, .. } if content == "first"));
        assert!(matches!(&nodes[2], Node::Text { content, .. } if content == "second"));

        let round_tripped = nodes_to_items(&nodes);
        assert_eq!(round_tripped.len(), 2);
        match &round_tripped[0] {
            Item::Message { id: _, extra_body, .. } => {
                assert_eq!(extra_body.get("first_only"), Some(&serde_json::json!("A")));
            }
            _ => panic!("expected message item"),
        }
        match &round_tripped[1] {
            Item::Message { id: _, extra_body, .. } => {
                assert!(extra_body.get("first_only").is_none());
            }
            _ => panic!("expected message item"),
        }
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

// ──────────────────────────────────────────────────────────────────────
// Compatibility bridge types.
//
// The canonical storage inside `UrpRequest` and `UrpResponse` is a flat
// `Vec<Node>`. The message/part bridge types below remain only for localized
// adapters and transforms that still need grouped compatibility views while the
// downstream reconstruction rules stay encoder-owned.
// ──────────────────────────────────────────────────────────────────────

/// Old role enum kept for compile compat.  `Tool` maps to nothing in flat
/// URP but some old callers still reference it during decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

impl From<OrdinaryRole> for Role {
    fn from(r: OrdinaryRole) -> Self {
        match r {
            OrdinaryRole::System => Role::System,
            OrdinaryRole::Developer => Role::Developer,
            OrdinaryRole::User => Role::User,
            OrdinaryRole::Assistant => Role::Assistant,
        }
    }
}

impl Role {
    pub fn to_ordinary(self) -> Option<OrdinaryRole> {
        match self {
            Role::System => Some(OrdinaryRole::System),
            Role::Developer => Some(OrdinaryRole::Developer),
            Role::User => Some(OrdinaryRole::User),
            Role::Assistant => Some(OrdinaryRole::Assistant),
            Role::Tool => None,
        }
    }
}

/// Old Part enum – each variant maps 1:1 to a Node variant minus `role`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        content: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Image {
        source: ImageSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Audio {
        source: AudioSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        source: FileSource,
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
    Refusal {
        content: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

/// Old Item enum – `Message` wraps role + parts, `ToolResult` is 1:1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    Message {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: Role,
        parts: Vec<Part>,
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
}

impl Item {
    pub fn new_message(role: Role) -> Self {
        Item::Message {
            id: None,
            role,
            parts: Vec::new(),
            extra_body: HashMap::new(),
        }
    }

    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Item::Message {
            id: None,
            role,
            parts: vec![Part::Text {
                content: content.into(),
                extra_body: HashMap::new(),
            }],
            extra_body: HashMap::new(),
        }
    }
}

/// Old stream header/delta types kept for compile compat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemHeader {
    Message {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: Role,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        call_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartHeader {
    Text,
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Refusal,
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        call_id: String,
        name: String,
    },
    Image {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Audio {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        item_type: String,
        body: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartDelta {
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

// ── Conversion helpers: Item/Part ↔ Node ──

impl Part {
    /// Convert a Part + role into a flat Node.
    pub fn into_node(self, role: OrdinaryRole) -> Node {
        match self {
            Part::Text { content, extra_body } => Node::Text {
                id: None,
                role,
                phase: extra_body
                    .get("phase")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                content,
                extra_body,
            },
            Part::Image { source, extra_body } => Node::Image { id: None, role, source, extra_body },
            Part::Audio { source, extra_body } => Node::Audio { id: None, role, source, extra_body },
            Part::File { source, extra_body } => Node::File { id: None, role, source, extra_body },
            Part::Reasoning { id, content, encrypted, summary, source, extra_body } => Node::Reasoning { id, content, encrypted, summary, source, extra_body },
            Part::ToolCall { id, call_id, name, arguments, extra_body } => Node::ToolCall { id, call_id, name, arguments, extra_body },
            Part::Refusal { content, extra_body } => Node::Refusal { id: None, content, extra_body },
            Part::ProviderItem { id, item_type, body, extra_body } => Node::ProviderItem { id, role, item_type, body, extra_body },
        }
    }
}

impl Item {
    /// Convert an Item into flat Node(s).
    pub fn into_nodes(self) -> Vec<Node> {
        match self {
            Item::Message { id, role, parts, extra_body } => {
                let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
                parts
                    .into_iter()
                    .enumerate()
                    .map(|(idx, p)| {
                        let mut node = p.into_node(ordinary_role);
                        if idx == 0 && !extra_body.is_empty() {
                            node.extra_body_mut().extend(extra_body.clone());
                        }
                        if idx == 0 {
                            node.set_id(id.clone());
                        }
                        node
                    })
                    .collect()
            }
            Item::ToolResult { id, call_id, is_error, content, extra_body } => {
                vec![Node::ToolResult { id, call_id, is_error, content, extra_body }]
            }
        }
    }

    /// Convert an Item into flat Node(s), preserving message-level envelope
    /// metadata as an explicit control node instead of attaching it to the
    /// first visible node.
    pub fn into_nodes_with_envelope_control(self) -> Vec<Node> {
        match self {
            Item::Message {
                id,
                role,
                parts,
                extra_body,
            } => {
                let ordinary_role = role.to_ordinary().unwrap_or(OrdinaryRole::User);
                let mut nodes = Vec::new();
                if !extra_body.is_empty() && !parts.is_empty() {
                    nodes.push(Node::NextDownstreamEnvelopeExtra { extra_body });
                }
                for (idx, part) in parts.into_iter().enumerate() {
                    let mut node = part.into_node(ordinary_role);
                    if idx == 0 {
                        node.set_id(id.clone());
                    }
                    nodes.push(node);
                }
                nodes
            }
            Item::ToolResult {
                id,
                call_id,
                is_error,
                content,
                extra_body,
            } => {
                vec![Node::ToolResult {
                    id,
                    call_id,
                    is_error,
                    content,
                    extra_body,
                }]
            }
        }
    }
}

/// Convert `Vec<Item>` (old grouped) into `Vec<Node>` (flat).
pub fn items_to_nodes(items: Vec<Item>) -> Vec<Node> {
    items.into_iter().flat_map(|item| item.into_nodes()).collect()
}

/// Convert `Vec<Item>` into `Vec<Node>` while preserving message-level
/// envelope metadata as explicit control nodes.
pub fn items_to_nodes_with_envelope_control(items: Vec<Item>) -> Vec<Node> {
    items.into_iter()
        .flat_map(|item| item.into_nodes_with_envelope_control())
        .collect()
}

/// Convert `Vec<Node>` (flat) into `Vec<Item>` (old grouped) for callers
/// that still expect the old shape.
pub fn nodes_to_items(nodes: &[Node]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut current_role: Option<Role> = None;
    let mut current_parts: Vec<Part> = Vec::new();
    let mut current_extra: HashMap<String, Value> = HashMap::new();
    let mut current_message_item_id: Option<String> = None;
    let mut current_phase: Option<String> = None;
    let mut current_zone: Option<BridgeZone> = None;
    let mut pending_control_extra: HashMap<String, Value> = HashMap::new();

    for node in nodes {
        match node {
            Node::ToolResult { id, call_id, is_error, content, extra_body } => {
                if !current_parts.is_empty() {
                    items.push(Item::Message {
                        id: current_message_item_id.take(),
                        role: current_role.unwrap_or(Role::User),
                        parts: std::mem::take(&mut current_parts),
                        extra_body: std::mem::take(&mut current_extra),
                    });
                    current_role = None;
                    current_phase = None;
                    current_zone = None;
                    current_message_item_id = None;
                }
                let mut merged_extra = extra_body.clone();
                for (key, value) in std::mem::take(&mut pending_control_extra) {
                    merged_extra.entry(key).or_insert(value);
                }
                items.push(Item::ToolResult {
                    id: id.clone(),
                    call_id: call_id.clone(),
                    is_error: *is_error,
                    content: content.clone(),
                    extra_body: merged_extra,
                });
            }
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                if !current_parts.is_empty() {
                    items.push(Item::Message {
                        id: current_message_item_id.take(),
                        role: current_role.unwrap_or(Role::User),
                        parts: std::mem::take(&mut current_parts),
                        extra_body: std::mem::take(&mut current_extra),
                    });
                    current_role = None;
                    current_phase = None;
                    current_zone = None;
                    current_message_item_id = None;
                }
                for (key, value) in extra_body {
                    pending_control_extra.insert(key.clone(), value.clone());
                }
            }
            _ => {
                let node_role: Role = node.role().map(Role::from).unwrap_or(Role::Assistant);
                let node_phase = match node {
                    Node::Text { phase, .. } => phase.clone(),
                    _ => None,
                };
                let node_zone = bridge_zone_for_node(node);
                let phased_content_boundary = current_role == Some(node_role)
                    && matches!(current_zone, Some(BridgeZone::Content))
                    && matches!(node_zone, BridgeZone::Content)
                    && current_phase != node_phase;
                let should_flush = current_role.is_some()
                    && (current_role != Some(node_role)
                        || phased_content_boundary
                        || bridge_zone_should_flush(current_zone, node_zone));
                if should_flush {
                    items.push(Item::Message {
                        id: current_message_item_id.take(),
                        role: current_role.unwrap_or(Role::User),
                        parts: std::mem::take(&mut current_parts),
                        extra_body: std::mem::take(&mut current_extra),
                    });
                    current_message_item_id = None;
                }
                if current_parts.is_empty() {
                    current_message_item_id = node.message_group_id();
                    current_extra = node.extra_body_for_message_boundary();
                    for (key, value) in std::mem::take(&mut pending_control_extra) {
                        current_extra.entry(key).or_insert(value);
                    }
                } else {
                    if !current_extra.contains_key("phase") {
                        if let Some(phase) = node_phase.as_ref() {
                            current_extra
                                .insert("phase".to_string(), Value::String(phase.clone()));
                        }
                    }
                }
                current_role = Some(node_role);
                current_phase = node_phase;
                current_zone = Some(node_zone);
                current_parts.push(node_to_part(node));
            }
        }
    }
    if !current_parts.is_empty() {
        items.push(Item::Message {
            id: current_message_item_id,
            role: current_role.unwrap_or(Role::User),
            parts: current_parts,
            extra_body: current_extra,
        });
    }
    items
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BridgeZone {
    Reasoning,
    Content,
    Action,
}

fn bridge_zone_for_node(node: &Node) -> BridgeZone {
    match node {
        Node::Reasoning { .. } => BridgeZone::Reasoning,
        Node::Text { .. }
        | Node::Image { .. }
        | Node::Audio { .. }
        | Node::File { .. }
        | Node::Refusal { .. } => BridgeZone::Content,
        Node::ToolCall { .. }
        | Node::ProviderItem { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => BridgeZone::Action,
    }
}

fn bridge_zone_should_flush(current: Option<BridgeZone>, next: BridgeZone) -> bool {
    match next {
        BridgeZone::Reasoning => matches!(current, Some(BridgeZone::Content | BridgeZone::Action)),
        BridgeZone::Content => matches!(current, Some(BridgeZone::Action)),
        BridgeZone::Action => false,
    }
}

fn node_to_part(node: &Node) -> Part {
    match node {
        Node::Text { content, extra_body, .. } => Part::Text { content: content.clone(), extra_body: extra_body.clone() },
        Node::Image { source, extra_body, .. } => Part::Image { source: source.clone(), extra_body: extra_body.clone() },
        Node::Audio { source, extra_body, .. } => Part::Audio { source: source.clone(), extra_body: extra_body.clone() },
        Node::File { source, extra_body, .. } => Part::File { source: source.clone(), extra_body: extra_body.clone() },
        Node::Reasoning { id, content, encrypted, summary, source, extra_body } => Part::Reasoning { id: id.clone(), content: content.clone(), encrypted: encrypted.clone(), summary: summary.clone(), source: source.clone(), extra_body: extra_body.clone() },
        Node::ToolCall { id, call_id, name, arguments, extra_body } => Part::ToolCall { id: id.clone(), call_id: call_id.clone(), name: name.clone(), arguments: arguments.clone(), extra_body: extra_body.clone() },
        Node::Refusal { content, extra_body, .. } => Part::Refusal { content: content.clone(), extra_body: extra_body.clone() },
        Node::ProviderItem { id, item_type, body, extra_body, .. } => Part::ProviderItem { id: id.clone(), item_type: item_type.clone(), body: body.clone(), extra_body: extra_body.clone() },
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => {
            // should not happen – caller should handle ToolResult separately
            Part::Text { content: String::new(), extra_body: HashMap::new() }
        }
    }
}

impl Node {
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

    fn message_group_id(&self) -> Option<String> {
        match self {
            Node::Text { id, .. }
            | Node::Image { id, .. }
            | Node::Audio { id, .. }
            | Node::File { id, .. }
            | Node::Refusal { id, .. } => id.clone(),
            _ => None,
        }
    }

    fn extra_body_for_message_boundary(&self) -> HashMap<String, Value> {
        match self {
            Node::Text { phase, extra_body, .. } => {
                let mut out = extra_body.clone();
                if let Some(phase) = phase {
                    out.insert("phase".to_string(), Value::String(phase.clone()));
                }
                out
            }
            Node::Image { extra_body, .. }
            | Node::Audio { extra_body, .. }
            | Node::File { extra_body, .. }
            | Node::Refusal { extra_body, .. }
            | Node::Reasoning { extra_body, .. }
            | Node::ToolCall { extra_body, .. }
            | Node::ProviderItem { extra_body, .. }
            | Node::ToolResult { extra_body, .. }
            | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body.clone(),
        }
    }
}

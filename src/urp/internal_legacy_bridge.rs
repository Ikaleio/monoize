use super::{
    AudioSource, FileSource, ImageSource, Node, OrdinaryRole, ProviderProtocol,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, ToolResultContent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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
        #[serde(default)]
        tool_type: super::ToolCallType,
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
        origin_protocol: ProviderProtocol,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

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
        #[serde(default)]
        tool_type: super::ToolCallType,
        call_id: String,
        #[serde(default)]
        is_error: bool,
        content: Vec<ToolResultContent>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

impl Part {
    pub fn into_node(self, role: OrdinaryRole) -> Node {
        match self {
            Part::Text {
                content,
                extra_body,
            } => Node::Text {
                id: None,
                role,
                phase: extra_body
                    .get("phase")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                content,
                extra_body,
            },
            Part::Image { source, extra_body } => Node::Image {
                id: None,
                role,
                source,
                extra_body,
            },
            Part::Audio { source, extra_body } => Node::Audio {
                id: None,
                role,
                source,
                extra_body,
            },
            Part::File { source, extra_body } => Node::File {
                id: None,
                role,
                source,
                extra_body,
            },
            Part::Reasoning {
                id,
                content,
                encrypted,
                summary,
                source,
                extra_body,
            } => Node::Reasoning {
                id,
                content,
                encrypted,
                summary,
                source,
                extra_body,
            },
            Part::ToolCall {
                id,
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
            } => Node::ToolCall {
                id,
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
            },
            Part::Refusal {
                content,
                extra_body,
            } => Node::Refusal {
                id: None,
                content,
                extra_body,
            },
            Part::ProviderItem {
                id,
                origin_protocol,
                item_type,
                body,
                extra_body,
            } => Node::ProviderItem {
                id,
                origin_protocol,
                role,
                item_type,
                body,
                extra_body,
            },
        }
    }
}

impl Item {}

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
            Node::ToolResult {
                id,
                tool_type,
                call_id,
                is_error,
                content,
                extra_body,
            } => {
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
                    tool_type: *tool_type,
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
                    current_message_item_id = message_group_id(node);
                    current_extra.clear();
                    for (key, value) in std::mem::take(&mut pending_control_extra) {
                        if !is_internal_marker(&key) {
                            current_extra.entry(key).or_insert(value);
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
        Node::Image { extra_body, .. }
            if extra_body.contains_key(RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY) =>
        {
            BridgeZone::Action
        }
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
        Node::Text {
            content,
            phase,
            extra_body,
            ..
        } => {
            let mut extra_body = extra_body.clone();
            if let Some(phase) = phase {
                extra_body.insert("phase".to_string(), Value::String(phase.clone()));
            }
            Part::Text {
                content: content.clone(),
                extra_body,
            }
        }
        Node::Image {
            source, extra_body, ..
        } => Part::Image {
            source: source.clone(),
            extra_body: extra_body.clone(),
        },
        Node::Audio {
            source, extra_body, ..
        } => Part::Audio {
            source: source.clone(),
            extra_body: extra_body.clone(),
        },
        Node::File {
            source, extra_body, ..
        } => Part::File {
            source: source.clone(),
            extra_body: extra_body.clone(),
        },
        Node::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => Part::Reasoning {
            id: id.clone(),
            content: content.clone(),
            encrypted: encrypted.clone(),
            summary: summary.clone(),
            source: source.clone(),
            extra_body: extra_body.clone(),
        },
        Node::ToolCall {
            id,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
        } => Part::ToolCall {
            id: id.clone(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            extra_body: extra_body.clone(),
        },
        Node::Refusal {
            content,
            extra_body,
            ..
        } => Part::Refusal {
            content: content.clone(),
            extra_body: extra_body.clone(),
        },
        Node::ProviderItem {
            id,
            origin_protocol,
            item_type,
            body,
            extra_body,
            ..
        } => Part::ProviderItem {
            id: id.clone(),
            origin_protocol: *origin_protocol,
            item_type: item_type.clone(),
            body: body.clone(),
            extra_body: extra_body.clone(),
        },
        Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => Part::Text {
            content: String::new(),
            extra_body: HashMap::new(),
        },
    }
}

fn message_group_id(node: &Node) -> Option<String> {
    match node {
        Node::Text { id, .. }
        | Node::Image { id, .. }
        | Node::Audio { id, .. }
        | Node::File { id, .. }
        | Node::Refusal { id, .. } => id.clone(),
        _ => None,
    }
}

fn is_internal_marker(key: &str) -> bool {
    key.starts_with("_monoize_")
}

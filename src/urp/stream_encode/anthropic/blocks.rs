//! The Messages content block of one node, built from its header or from its complete state.

use crate::urp::encode::{merge_extra, sanitize_provider_item_wire_body};
use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, Citation, MESSAGES_CHAT_REASONING_DETAIL_TYPE_KEY, Node,
    NodeDelta, NodeHeader, ProviderProtocol, REASONING_ENVELOPE_PREFIX, ReasoningMetadata,
    ToolCallType, is_internal_key, tool_call_arguments_for_wire,
    wrap_reasoning_signature_with_item_id,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// The payload of one Messages content block. A live block accumulates it from deltas.
pub(super) enum BlockPayload {
    Text {
        content: String,
        citations: Vec<Citation>,
        phase: Option<String>,
        extra: HashMap<String, Value>,
    },
    Thinking {
        metadata: ReasoningMetadata,
        thinking: String,
        /// The raw signature; `wire_signature` applies the DM5.2 item-id envelope.
        signature: Option<String>,
        item_id: Option<String>,
        extra: HashMap<String, Value>,
    },
    ToolUse {
        namespace: Option<String>,
        call_id: String,
        name: String,
        arguments: String,
        extra: HashMap<String, Value>,
    },
    ProviderItem {
        body: Value,
        deltas: Vec<Value>,
    },
}

impl BlockPayload {
    /// The empty block a `NodeStart` announces; `None` for a node without a Messages block.
    pub(super) fn from_header(
        header: &NodeHeader,
        extra_body: &HashMap<String, Value>,
    ) -> Option<Self> {
        match header {
            NodeHeader::Text {
                citations, phase, ..
            } => Some(Self::Text {
                content: String::new(),
                citations: citations.clone(),
                phase: phase.clone(),
                extra: extra_body.clone(),
            }),
            NodeHeader::Refusal { .. } => Some(Self::Text {
                content: String::new(),
                citations: Vec::new(),
                phase: None,
                extra: extra_body.clone(),
            }),
            NodeHeader::Reasoning { metadata, .. } => Some(Self::Thinking {
                metadata: metadata.clone(),
                thinking: String::new(),
                signature: None,
                item_id: None,
                extra: thinking_extra(extra_body),
            }),
            NodeHeader::ToolCall {
                namespace,
                tool_type: ToolCallType::Function,
                call_id,
                name,
                ..
            } => Some(Self::ToolUse {
                namespace: namespace.clone(),
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: String::new(),
                extra: extra_body.clone(),
            }),
            NodeHeader::ProviderItem {
                id,
                origin_protocol: ProviderProtocol::Messages,
                body,
                item_type,
                ..
            } => Some(Self::ProviderItem {
                body: provider_start_body(id.as_ref(), item_type, body.as_ref(), extra_body),
                deltas: Vec::new(),
            }),
            NodeHeader::ToolCall { .. }
            | NodeHeader::Image { .. }
            | NodeHeader::Audio { .. }
            | NodeHeader::File { .. }
            | NodeHeader::ProviderItem { .. }
            | NodeHeader::ToolResult { .. }
            | NodeHeader::NextDownstreamEnvelopeExtra => None,
        }
    }

    /// The complete block of `node`; `None` for a node without a Messages block (DM5.1 case 4,
    /// MSG-7).
    pub(super) fn from_node(node: &Node) -> Option<Self> {
        match node {
            Node::Text {
                content,
                citations,
                phase,
                extra_body,
                ..
            } => Some(Self::Text {
                content: content.clone(),
                citations: citations.clone(),
                phase: phase.clone(),
                extra: extra_body.clone(),
            }),
            Node::Refusal { content, .. } => Some(Self::Text {
                content: content.clone(),
                citations: Vec::new(),
                phase: None,
                extra: HashMap::new(),
            }),
            Node::Reasoning {
                metadata,
                id,
                content,
                summary,
                encrypted,
                extra_body,
                ..
            } => {
                let thinking = non_empty(content.as_deref())
                    .or_else(|| non_empty(summary.as_deref()))
                    .unwrap_or_default();
                let signature = signature_text(encrypted.as_ref());
                if signature.is_none() && (metadata.redacted || thinking.is_empty()) {
                    return None;
                }
                Some(Self::Thinking {
                    metadata: metadata.clone(),
                    thinking: thinking.to_string(),
                    signature,
                    item_id: id.clone().filter(|id| !id.is_empty()),
                    extra: thinking_extra(extra_body),
                })
            }
            Node::ToolCall {
                namespace,
                tool_type: ToolCallType::Function,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => Some(Self::ToolUse {
                namespace: namespace.clone(),
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: tool_call_arguments_for_wire(arguments),
                extra: extra_body.clone(),
            }),
            Node::ProviderItem { .. } => {
                messages_provider_block(node).map(|body| Self::ProviderItem {
                    body,
                    deltas: Vec::new(),
                })
            }
            Node::ToolCall { .. }
            | Node::Image { .. }
            | Node::Audio { .. }
            | Node::File { .. }
            | Node::ToolResult { .. }
            | Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    pub(super) fn is_tool_use(&self) -> bool {
        matches!(self, Self::ToolUse { .. })
    }

    /// The `content_block` of `content_block_start`. Text, thinking, signatures, and tool
    /// input follow as deltas (DM5b); a redacted block carries its payload here.
    pub(super) fn start_block(&self) -> Value {
        match self {
            Self::Text { phase, extra, .. } => {
                let mut block = Map::from_iter([
                    ("type".to_string(), json!("text")),
                    ("text".to_string(), json!("")),
                ]);
                if let Some(phase) = phase {
                    block.insert("phase".to_string(), json!(phase));
                }
                merge_extra(&mut block, extra);
                Value::Object(block)
            }
            Self::Thinking {
                metadata, extra, ..
            } => {
                let mut block = if metadata.redacted {
                    Map::from_iter([
                        ("type".to_string(), json!("redacted_thinking")),
                        (
                            "data".to_string(),
                            json!(self.wire_signature().unwrap_or_default()),
                        ),
                    ])
                } else {
                    Map::from_iter([
                        ("type".to_string(), json!("thinking")),
                        ("thinking".to_string(), json!("")),
                        ("signature".to_string(), json!("")),
                    ])
                };
                merge_extra(&mut block, extra);
                Value::Object(block)
            }
            Self::ToolUse {
                namespace,
                call_id,
                name,
                extra,
                ..
            } => {
                let mut block = Map::from_iter([
                    ("type".to_string(), json!("tool_use")),
                    ("id".to_string(), json!(call_id)),
                    ("name".to_string(), json!(name)),
                    ("input".to_string(), json!({})),
                ]);
                merge_extra(&mut block, extra);
                // The typed namespace owns `toolset_name`.
                block.remove("toolset_name");
                if let Some(namespace) = namespace {
                    block.insert("toolset_name".to_string(), json!(namespace));
                }
                Value::Object(block)
            }
            Self::ProviderItem { body, .. } => body.clone(),
        }
    }

    /// The signature of a complete thinking block (DM5.2), `None` when it has none.
    pub(super) fn wire_signature(&self) -> Option<String> {
        let Self::Thinking {
            signature, item_id, ..
        } = self
        else {
            return None;
        };
        let raw = signature
            .as_deref()
            .filter(|signature| !signature.is_empty())?;
        Some(wire_signature(raw, item_id.as_deref()))
    }

    /// DM5.3 item 5: a thinking block streams once it has text or a signature. A redacted
    /// block carries its payload in `content_block_start`, so it never opens before completion.
    pub(super) fn opens_live(&self) -> bool {
        match self {
            Self::Thinking {
                metadata,
                thinking,
                signature,
                ..
            } => {
                !metadata.redacted
                    && (!thinking.is_empty() || signature.as_deref().is_some_and(|s| !s.is_empty()))
            }
            _ => true,
        }
    }

    /// Applies the reasoning metadata of `delta` before the delta is encoded, so its redaction
    /// flag and its call-signature item id govern the delta itself.
    pub(super) fn merge_delta_metadata(&mut self, delta: &NodeDelta) {
        if let (
            Self::Thinking {
                metadata, item_id, ..
            },
            NodeDelta::Reasoning {
                metadata: delta_metadata,
                ..
            },
        ) = (self, delta)
        {
            metadata.merge(delta_metadata);
            if let Some(id) = &delta_metadata.item_id {
                *item_id = Some(id.clone());
            }
        }
    }

    /// Adds `delta` to the payload. A block whose delta reached the wire tracks only what it
    /// sent (DM5a); a waiting block keeps the first summary text as its thinking text.
    pub(super) fn absorb_delta(&mut self, delta: &NodeDelta, emitted: bool) {
        match (self, delta) {
            (
                Self::Text {
                    content, citations, ..
                },
                NodeDelta::Text {
                    content: text,
                    citations: added,
                    ..
                },
            ) => {
                content.push_str(text);
                citations.extend(added.iter().cloned());
            }
            (Self::Text { content, .. }, NodeDelta::Refusal { content: text, .. }) => {
                content.push_str(text);
            }
            (
                Self::Thinking {
                    thinking,
                    signature,
                    ..
                },
                NodeDelta::Reasoning {
                    metadata,
                    content,
                    encrypted,
                    summary,
                    ..
                },
            ) => {
                let summary_is_thinking = if emitted {
                    metadata.summary_as_thinking
                } else {
                    thinking.is_empty()
                };
                if let Some(text) = non_empty(content.as_deref()) {
                    thinking.push_str(text);
                } else if summary_is_thinking && let Some(text) = non_empty(summary.as_deref()) {
                    thinking.push_str(text);
                }
                if let Some(fragment) = signature_text(encrypted.as_ref()) {
                    signature
                        .get_or_insert_with(String::new)
                        .push_str(&fragment);
                }
            }
            (
                Self::ToolUse { arguments, .. },
                NodeDelta::ToolCallArguments {
                    arguments: fragment,
                },
            ) => {
                arguments.push_str(fragment);
            }
            (Self::ProviderItem { deltas, .. }, NodeDelta::ProviderItem { data }) => {
                deltas.push(data.clone());
            }
            _ => {}
        }
    }

    /// DM5a.1: this Chat `reasoning.text` block with plaintext and no signature takes the
    /// signature of `following`, an encrypted-only `reasoning.encrypted` node.
    pub(super) fn takes_signature_of(&self, following: &Self) -> bool {
        let (
            Self::Thinking {
                metadata,
                thinking,
                signature,
                extra,
                ..
            },
            Self::Thinking {
                metadata: following_metadata,
                thinking: following_thinking,
                signature: following_signature,
                extra: following_extra,
                ..
            },
        ) = (self, following)
        else {
            return false;
        };
        !metadata.redacted
            && !following_metadata.redacted
            && !thinking.is_empty()
            && signature.as_deref().is_none_or(str::is_empty)
            && chat_detail_type(extra) == Some("reasoning.text")
            && following_thinking.is_empty()
            && following_signature
                .as_deref()
                .is_some_and(|s| !s.is_empty())
            && chat_detail_type(following_extra) == Some("reasoning.encrypted")
    }
}

pub(super) fn non_empty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

/// A non-empty signature payload as wire text; a non-string payload becomes its JSON text.
pub(super) fn signature_text(encrypted: Option<&Value>) -> Option<String> {
    encrypted
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned)
        })
        .filter(|signature| !signature.is_empty())
}

/// DM5.2: a downstream signature wraps the raw payload with the reasoning item id, unless it
/// already is an envelope.
pub(super) fn wire_signature(raw: &str, item_id: Option<&str>) -> String {
    if raw.starts_with(REASONING_ENVELOPE_PREFIX) {
        return raw.to_string();
    }
    item_id
        .filter(|id| !id.is_empty())
        .and_then(|id| wrap_reasoning_signature_with_item_id(id, raw))
        .unwrap_or_else(|| raw.to_string())
}

/// The complete wire block of a Messages provider item; `None` for another origin or for a
/// body that is not an object.
pub(super) fn messages_provider_block(node: &Node) -> Option<Value> {
    let Node::ProviderItem {
        id,
        origin_protocol: ProviderProtocol::Messages,
        item_type,
        body,
        extra_body,
        ..
    } = node
    else {
        return None;
    };
    let Value::Object(mut block) = sanitize_provider_item_wire_body(body) else {
        return None;
    };
    set_provider_identity(&mut block, id.as_ref(), item_type);
    merge_extra(&mut block, extra_body);
    Some(Value::Object(block))
}

/// The tool input JSON a provider item block carries: its `input` member, replaced by the
/// first `input_json_delta` when that member is absent or empty, then extended by later ones.
pub(super) fn provider_input_json(body: &Value, deltas: &[Value]) -> Option<String> {
    let input = body.get("input");
    let mut assembled = match input {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    };
    let mut replace_on_next_delta = matches!(input, None | Some(Value::Null))
        || input.and_then(Value::as_object).is_some_and(Map::is_empty);
    let mut saw_delta = false;
    let fragments = deltas
        .iter()
        .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("input_json_delta"))
        .filter_map(|delta| delta.get("partial_json").and_then(Value::as_str));
    for fragment in fragments {
        if replace_on_next_delta {
            assembled.clear();
            replace_on_next_delta = false;
        }
        assembled.push_str(fragment);
        saw_delta = true;
    }
    (saw_delta || input.is_some()).then_some(assembled)
}

/// The `content_block_start` body of a provider item from its header.
fn provider_start_body(
    id: Option<&String>,
    item_type: &str,
    body: Option<&Value>,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut body = match body {
        Some(body) => sanitize_provider_item_wire_body(body),
        None => {
            let mut block = Map::new();
            block.insert("type".to_string(), json!(item_type));
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                block.insert("id".to_string(), json!(id));
            }
            merge_extra(&mut block, extra_body);
            Value::Object(block)
        }
    };
    if let Some(block) = body.as_object_mut() {
        set_provider_identity(block, id, item_type);
    }
    body
}

/// The typed `item_type` and `id` own the block kind and identity. A block without an `id`
/// member keeps none.
fn set_provider_identity(block: &mut Map<String, Value>, id: Option<&String>, item_type: &str) {
    block.insert("type".to_string(), json!(item_type));
    if block.remove("id").is_some()
        && let Some(id) = id
    {
        block.insert("id".to_string(), json!(id));
    }
}

/// The wire members of a reasoning node plus the Chat detail type that DM5a.1 pairing reads.
fn thinking_extra(extra_body: &HashMap<String, Value>) -> HashMap<String, Value> {
    let mut extra: HashMap<String, Value> = extra_body
        .iter()
        .filter(|(key, _)| !is_internal_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(detail_type) = extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
        .and_then(|detail| detail.get("type"))
        .and_then(Value::as_str)
        .filter(|detail_type| !detail_type.is_empty())
    {
        extra.insert(
            MESSAGES_CHAT_REASONING_DETAIL_TYPE_KEY.to_string(),
            json!(detail_type),
        );
    }
    extra
}

fn chat_detail_type(extra: &HashMap<String, Value>) -> Option<&str> {
    extra
        .get(MESSAGES_CHAT_REASONING_DETAIL_TYPE_KEY)
        .and_then(Value::as_str)
}

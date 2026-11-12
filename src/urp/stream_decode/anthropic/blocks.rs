//! One open Messages content block: its start events, delta accumulation, and terminal node.

use super::json_input::{
    CustomToolInputDecoder, PartialJsonInput, input_is_placeholder, json_value_to_string,
};
use super::object_without_keys;
use crate::urp::{
    Citation, Node, NodeDelta, NodeHeader, OrdinaryRole, ProviderProtocol, ReasoningMetadata,
    ToolCallType, UrpStreamEvent, unwrap_reasoning_signature_sigil,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

type BlockDelta = (NodeDelta, HashMap<String, Value>);

struct ParsedDelta<'a> {
    kind: &'a str,
    payload: &'a str,
    citation: Option<&'a Value>,
    extra: HashMap<String, Value>,
}

impl<'a> ParsedDelta<'a> {
    fn parse(delta: &'a Value) -> Result<Self, String> {
        let kind = delta.get("type").and_then(Value::as_str).unwrap_or("");
        let payload_key = match kind {
            "text_delta" => Some("text"),
            "thinking_delta" => Some("thinking"),
            "signature_delta" => Some("signature"),
            "input_json_delta" => Some("partial_json"),
            _ => None,
        };
        let payload = match payload_key {
            Some(key) => delta
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Messages {kind} requires a string payload"))?,
            None => "",
        };
        let citation = delta.get("citation").filter(|value| value.is_object());
        if kind == "citations_delta" && citation.is_none() {
            return Err("Messages citations_delta requires a citation object".to_string());
        }
        Ok(Self {
            kind,
            payload,
            citation,
            extra: object_without_keys(
                delta,
                &["type", "text", "thinking", "signature", "partial_json"],
            ),
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct ActiveBlock {
    kind: BlockKind,
    extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
enum BlockKind {
    /// A block whose start event already carries the complete node (media, tool results).
    Complete(Box<Node>),
    Text {
        citations: Vec<Citation>,
        content: String,
        phase: Option<String>,
    },
    Reasoning {
        metadata: ReasoningMetadata,
        summary: String,
        encrypted: String,
    },
    ToolCall {
        namespace: Option<String>,
        tool_type: ToolCallType,
        call_id: String,
        name: String,
        arguments: String,
        replace_on_next_delta: bool,
        custom_input: Option<CustomToolInputDecoder>,
    },
    ProviderItem {
        id: Option<String>,
        item_type: String,
        body: Value,
        input: PartialJsonInput,
    },
}

/// The URP error code for a `content_block_start` that fails to decode.
pub(super) fn start_error_code(content_block: &Value) -> &'static str {
    match content_block.get("type").and_then(Value::as_str) {
        Some(
            "image" | "image_url" | "input_image" | "output_image" | "document" | "file"
            | "input_file" | "output_file" | "audio" | "input_audio" | "output_audio",
        ) => "messages_media_content_invalid",
        Some("tool_result") => "messages_tool_result_content_invalid",
        _ => "messages_custom_tool_input_invalid",
    }
}

impl ActiveBlock {
    pub(super) fn from_content_block(
        content_block: &Value,
        custom_tool_names: &HashSet<String>,
    ) -> Result<Option<Self>, String> {
        if let Some(text) = content_block.as_str() {
            return Ok(Some(Self {
                kind: BlockKind::Text {
                    citations: Vec::new(),
                    content: text.to_string(),
                    phase: None,
                },
                extra_body: HashMap::new(),
            }));
        }
        let content_type = content_block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let string_member = |key: &str| {
            content_block
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
        };

        Ok(match content_type {
            "text" | "input_text" | "output_text" => {
                let citations = content_block.get("citations").and_then(Value::as_array);
                let mut extra_body =
                    object_without_keys(content_block, &["type", "text", "phase", "citations"]);
                // PM6e: native text starts carry `citations: []`; typed citations follow as deltas.
                if citations.is_some_and(Vec::is_empty) {
                    extra_body.insert("citations".to_string(), Value::Array(Vec::new()));
                }
                Some(Self {
                    kind: BlockKind::Text {
                        citations: crate::urp::citations::decode(
                            citations.cloned().unwrap_or_default(),
                            ProviderProtocol::Messages,
                        ),
                        content: string_member("text").unwrap_or_default(),
                        phase: string_member("phase"),
                    },
                    extra_body,
                })
            }
            "thinking" => Some(Self {
                kind: BlockKind::Reasoning {
                    metadata: ReasoningMetadata {
                        summary_as_thinking: true,
                        ..Default::default()
                    },
                    summary: string_member("thinking").unwrap_or_default(),
                    encrypted: string_member("signature").unwrap_or_default(),
                },
                extra_body: object_without_keys(content_block, &["type", "thinking", "signature"]),
            }),
            "redacted_thinking" => Some(Self {
                kind: BlockKind::Reasoning {
                    metadata: ReasoningMetadata {
                        redacted: true,
                        ..Default::default()
                    },
                    summary: String::new(),
                    encrypted: string_member("data").unwrap_or_default(),
                },
                extra_body: object_without_keys(content_block, &["type", "data"]),
            }),
            "tool_use" => Some(Self::from_tool_use(content_block, custom_tool_names)?),
            "image" | "image_url" | "input_image" | "output_image" | "document" | "file"
            | "input_file" | "output_file" | "audio" | "input_audio" | "output_audio"
            | "tool_result" => crate::urp::decode::anthropic::decode_content_block(
                content_block,
                OrdinaryRole::Assistant,
            )?
            .map(|mut node| Self {
                extra_body: node.extra_body_mut().clone(),
                kind: BlockKind::Complete(Box::new(node)),
            }),
            _ => Some(Self {
                kind: BlockKind::ProviderItem {
                    id: string_member("id")
                        .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
                    item_type: content_type.to_string(),
                    body: content_block.clone(),
                    input: PartialJsonInput::from_content_block(content_block),
                },
                extra_body: HashMap::new(),
            }),
        })
    }

    fn from_tool_use(
        content_block: &Value,
        custom_tool_names: &HashSet<String>,
    ) -> Result<Self, String> {
        let string_member = |key: &str| {
            content_block
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let name = string_member("name").unwrap_or_default();
        let namespace = string_member("toolset_name");
        let input = content_block.get("input");
        let placeholder = input_is_placeholder(input);
        let tool_type = if namespace.is_none() && custom_tool_names.contains(&name) {
            ToolCallType::Custom
        } else {
            ToolCallType::Function
        };
        let (arguments, custom_input) = if tool_type == ToolCallType::Custom {
            let mut decoder = CustomToolInputDecoder::new();
            let arguments = if placeholder {
                String::new()
            } else {
                decoder.push_fragment(&json_value_to_string(input))?
            };
            (arguments, Some(decoder))
        } else {
            (json_value_to_string(input), None)
        };
        Ok(Self {
            kind: BlockKind::ToolCall {
                namespace,
                tool_type,
                call_id: string_member("id").unwrap_or_default(),
                name,
                arguments,
                replace_on_next_delta: placeholder,
                custom_input,
            },
            extra_body: object_without_keys(
                content_block,
                &["type", "id", "name", "input", "toolset_name"],
            ),
        })
    }

    /// `NodeStart`, followed by a delta for any payload the start block already carries.
    pub(super) fn start_events(&self, node_index: u32) -> Vec<UrpStreamEvent> {
        let start = |header: NodeHeader| UrpStreamEvent::NodeStart {
            node_index,
            header,
            extra_body: self.extra_body.clone(),
        };
        let node_delta =
            |delta: NodeDelta, extra_body: HashMap<String, Value>| UrpStreamEvent::NodeDelta {
                node_index,
                delta,
                usage: None,
                extra_body,
            };
        let mut events = Vec::with_capacity(2);
        match &self.kind {
            BlockKind::Complete(node) => {
                events.push(start(NodeHeader::from(node.as_ref())));
                let media = match node.as_ref() {
                    Node::Image { source, .. } => Some(NodeDelta::Image {
                        source: source.clone(),
                    }),
                    Node::File { source, .. } => Some(NodeDelta::File {
                        source: source.clone(),
                    }),
                    Node::Audio { source, .. } => Some(NodeDelta::Audio {
                        source: source.clone(),
                    }),
                    _ => None,
                };
                events.extend(media.map(|media| node_delta(media, HashMap::new())));
            }
            _ => {
                let node = self.clone().into_node();
                events.push(start(NodeHeader::from(&node)));
                if let Node::ToolCall {
                    tool_type: ToolCallType::Custom,
                    arguments,
                    ..
                } = node
                    && !arguments.is_empty()
                {
                    events.push(node_delta(
                        NodeDelta::ToolCallArguments { arguments },
                        self.extra_body.clone(),
                    ));
                }
            }
        }
        events
    }

    /// Accumulates one `content_block_delta.delta` and returns the URP delta with its
    /// unknown members, or `None` when the delta carries nothing to forward.
    pub(super) fn apply_delta(&mut self, delta: &Value) -> Result<Option<BlockDelta>, String> {
        let ParsedDelta {
            kind: delta_type,
            payload,
            citation,
            extra: mut delta_extra,
        } = ParsedDelta::parse(delta)?;

        let node_delta = match (&mut self.kind, delta_type) {
            (BlockKind::Text { citations, .. }, "citations_delta") => {
                let citation = Citation::decode(
                    citation.cloned().unwrap_or_default(),
                    ProviderProtocol::Messages,
                );
                citations.push(citation.clone());
                delta_extra.remove("citation");
                NodeDelta::Text {
                    logprobs: None,
                    signature: None,
                    citations: vec![citation],
                    content: String::new(),
                }
            }
            (BlockKind::Text { content, .. }, "text_delta") => {
                if payload.is_empty() {
                    return Ok(None);
                }
                content.push_str(payload);
                NodeDelta::Text {
                    logprobs: None,
                    signature: None,
                    citations: Vec::new(),
                    content: payload.to_string(),
                }
            }
            (BlockKind::Reasoning { summary, .. }, "thinking_delta") => {
                if payload.is_empty()
                    && delta_extra
                        .get("estimated_tokens")
                        .is_none_or(Value::is_null)
                {
                    return Ok(None);
                }
                summary.push_str(payload);
                NodeDelta::Reasoning {
                    metadata: ReasoningMetadata {
                        summary_as_thinking: true,
                        ..Default::default()
                    },
                    content: None,
                    encrypted: None,
                    summary: (!payload.is_empty()).then(|| payload.to_string()),
                    source: None,
                }
            }
            (
                BlockKind::Reasoning {
                    metadata,
                    encrypted,
                    ..
                },
                "signature_delta",
            ) => {
                if payload.is_empty() {
                    return Ok(None);
                }
                encrypted.push_str(payload);
                NodeDelta::Reasoning {
                    metadata: metadata.clone(),
                    content: None,
                    encrypted: Some(Value::String(payload.to_string())),
                    summary: None,
                    source: None,
                }
            }
            (
                BlockKind::ToolCall {
                    arguments,
                    replace_on_next_delta,
                    custom_input,
                    ..
                },
                "input_json_delta",
            ) => {
                let Some(delta) =
                    accumulate_tool_input(arguments, replace_on_next_delta, custom_input, payload)?
                else {
                    return Ok(None);
                };
                delta
            }
            (BlockKind::ProviderItem { body, input, .. }, _) => {
                if delta_type == "compaction_delta"
                    && let Some(content) = delta.get("content")
                {
                    body["content"] = content.clone();
                }
                input.merge_delta(delta);
                NodeDelta::ProviderItem {
                    data: delta.clone(),
                }
            }
            (
                _,
                "text_delta" | "thinking_delta" | "signature_delta" | "input_json_delta"
                | "citations_delta" | "compaction_delta",
            ) => {
                return Err(format!(
                    "Messages {delta_type} does not match its active content block"
                ));
            }
            _ => return Ok(None),
        };
        Ok(Some((node_delta, delta_extra)))
    }

    /// The terminal node and the `NodeDone` extras of a closed block.
    pub(super) fn finish(self) -> Result<(Node, HashMap<String, Value>), String> {
        if let BlockKind::ToolCall {
            custom_input: Some(decoder),
            ..
        } = &self.kind
        {
            decoder.finish()?;
        }
        let extra_body = self.extra_body.clone();
        Ok((self.into_node(), extra_body))
    }

    fn into_node(self) -> Node {
        let extra_body = self.extra_body;
        match self.kind {
            BlockKind::Complete(node) => *node,
            BlockKind::Text {
                citations,
                content,
                phase,
            } => Node::Text {
                logprobs: None,
                signature: None,
                citations,
                id: None,
                role: OrdinaryRole::Assistant,
                content,
                phase,
                extra_body,
                cache_breakpoint: None,
            },
            BlockKind::Reasoning {
                metadata,
                summary,
                encrypted,
            } => {
                let (id, encrypted) = if encrypted.is_empty() {
                    (None, None)
                } else {
                    match unwrap_reasoning_signature_sigil(&encrypted) {
                        Some((id, original)) => (Some(id), Some(Value::String(original))),
                        None => (None, Some(Value::String(encrypted))),
                    }
                };
                Node::Reasoning {
                    metadata: ReasoningMetadata {
                        downstream_only: !summary.is_empty() && id.is_none() && encrypted.is_none(),
                        ..metadata
                    },
                    id,
                    content: None,
                    encrypted,
                    summary: (!summary.is_empty()).then_some(summary),
                    source: None,
                    extra_body,
                }
            }
            BlockKind::ToolCall {
                namespace,
                tool_type,
                call_id,
                name,
                arguments,
                ..
            } => Node::ToolCall {
                namespace,
                signature: None,
                id: Some(call_id.clone()),
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                cache_breakpoint: None,
            },
            BlockKind::ProviderItem {
                id,
                item_type,
                body,
                input,
            } => Node::ProviderItem {
                id,
                origin_protocol: ProviderProtocol::Messages,
                role: OrdinaryRole::Assistant,
                item_type,
                body: input.apply_to(body),
                extra_body,
            },
        }
    }
}

fn accumulate_tool_input(
    arguments: &mut String,
    replace_on_next_delta: &mut bool,
    custom_input: &mut Option<CustomToolInputDecoder>,
    payload: &str,
) -> Result<Option<NodeDelta>, String> {
    if payload.is_empty() {
        return Ok(None);
    }
    let fragment = match custom_input {
        Some(decoder) => {
            let decoded = decoder.push_fragment(payload)?;
            if decoded.is_empty() {
                return Ok(None);
            }
            decoded
        }
        None => {
            if *replace_on_next_delta {
                arguments.clear();
                *replace_on_next_delta = false;
            }
            payload.to_string()
        }
    };
    arguments.push_str(&fragment);
    Ok(Some(NodeDelta::ToolCallArguments {
        arguments: fragment,
    }))
}

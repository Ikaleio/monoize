use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Deserialize)]
struct Config {
    tag: String,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ReasoningToThinkXmlTransform;

#[async_trait]
impl Transform for ReasoningToThinkXmlTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_to_think_xml"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: to think XML"),
            ("zh", "推理：转为 think XML"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Converts reasoning node content into assistant text wrapped in the configured think tag. Inverse of reasoning_from_think_xml.",
            ),
            (
                "zh",
                "将推理节点内容转换为使用配置标签包裹的 assistant 文本。与 reasoning_from_think_xml 互逆。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[TransformScope::Provider, TransformScope::ApiKey]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "tag": { "type": "string", "minLength": 1 } },
            "required": ["tag"],
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        Ok(Box::new(cfg))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(StreamState::default())
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        match data {
            UrpData::Response(resp) => {
                for node in &mut resp.output {
                    if let Some(converted) = convert_node(node, &cfg.tag) {
                        *node = converted;
                    }
                }
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<StreamState>() else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                stream_state.apply(event, &cfg.tag);
            }
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlainKind {
    Content,
    Summary,
}

enum NodeMode {
    /// `NodeStart` withheld until the first delta decides the mode (TXO-3).
    Pending(UrpStreamEvent),
    Converted {
        kind: PlainKind,
        text: String,
        id: Option<String>,
    },
    PassThrough,
    /// `C` already emitted; the node's later events are dropped (TXO-3).
    Closed,
}

#[derive(Default)]
struct StreamState {
    nodes: HashMap<u32, NodeMode>,
    closed: HashMap<u32, Node>,
    passed_through: HashSet<u32>,
    replacement: Option<Vec<UrpStreamEvent>>,
}

impl TransformState for StreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn finalize_stream_event(&mut self, event: UrpStreamEvent) -> Vec<UrpStreamEvent> {
        self.replacement.take().unwrap_or_else(|| vec![event])
    }
}

fn plain_text(content: Option<&str>, summary: Option<&str>) -> Option<(PlainKind, String)> {
    content
        .filter(|content| !content.is_empty())
        .map(|content| (PlainKind::Content, content.to_string()))
        .or_else(|| {
            summary
                .filter(|summary| !summary.is_empty())
                .map(|summary| (PlainKind::Summary, summary.to_string()))
        })
}

fn text_node(id: Option<String>, content: String) -> Node {
    Node::Text {
        logprobs: None,
        signature: None,
        citations: Vec::new(),
        id,
        role: OrdinaryRole::Assistant,
        content,
        phase: None,
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }
}

fn text_header(id: Option<String>) -> NodeHeader {
    NodeHeader::Text {
        signature: None,
        citations: Vec::new(),
        id,
        role: OrdinaryRole::Assistant,
        phase: None,
    }
}

fn text_delta(node_index: u32, content: String) -> UrpStreamEvent {
    UrpStreamEvent::NodeDelta {
        node_index,
        delta: NodeDelta::Text {
            logprobs: None,
            signature: None,
            citations: Vec::new(),
            content,
        },
        usage: None,
        extra_body: HashMap::new(),
    }
}

/// TXO-2: a reasoning node with plain text becomes one tagged assistant text node.
fn convert_node(node: &Node, tag: &str) -> Option<Node> {
    let Node::Reasoning {
        id,
        content,
        summary,
        ..
    } = node
    else {
        return None;
    };
    let (_, plain) = plain_text(content.as_deref(), summary.as_deref())?;
    Some(text_node(id.clone(), format!("<{tag}>{plain}</{tag}>")))
}

impl StreamState {
    /// TXO-3: closes every open converted node except `keep`, in node order.
    fn close_open_nodes(&mut self, keep: Option<u32>, tag: &str) -> Vec<UrpStreamEvent> {
        let mut open: Vec<u32> = self
            .nodes
            .iter()
            .filter(|(index, mode)| {
                Some(**index) != keep && matches!(mode, NodeMode::Converted { .. })
            })
            .map(|(index, _)| *index)
            .collect();
        open.sort_unstable();
        let mut out = Vec::with_capacity(open.len() * 2);
        for node_index in open {
            out.extend(self.close_node(node_index, tag));
        }
        out
    }

    fn close_node(&mut self, node_index: u32, tag: &str) -> Vec<UrpStreamEvent> {
        let Some(NodeMode::Converted { text, id, .. }) =
            self.nodes.insert(node_index, NodeMode::Closed)
        else {
            return Vec::new();
        };
        let node = text_node(id, format!("<{tag}>{text}</{tag}>"));
        self.closed.insert(node_index, node.clone());
        vec![
            text_delta(node_index, format!("</{tag}>")),
            UrpStreamEvent::NodeDone {
                node_index,
                node,
                usage: None,
                extra_body: HashMap::new(),
            },
        ]
    }

    fn apply(&mut self, event: &mut UrpStreamEvent, tag: &str) {
        let closing = match event {
            UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                ..
            } => Vec::new(),
            UrpStreamEvent::NodeStart { node_index, .. }
            | UrpStreamEvent::NodeDelta { node_index, .. }
            | UrpStreamEvent::NodeDone { node_index, .. } => {
                self.close_open_nodes(Some(*node_index), tag)
            }
            UrpStreamEvent::ResponseDone { .. } => self.close_open_nodes(None, tag),
            _ => Vec::new(),
        };
        self.convert_event(event, tag);
        if !closing.is_empty() {
            let rest = self
                .replacement
                .take()
                .unwrap_or_else(|| vec![event.clone()]);
            self.replacement = Some(closing.into_iter().chain(rest).collect());
        }
    }

    fn convert_event(&mut self, event: &mut UrpStreamEvent, tag: &str) {
        match event {
            UrpStreamEvent::NodeStart {
                node_index,
                header: NodeHeader::Reasoning { .. },
                ..
            } => {
                let node_index = *node_index;
                self.nodes
                    .insert(node_index, NodeMode::Pending(event.clone()));
                self.replacement = Some(Vec::new());
            }
            UrpStreamEvent::NodeDelta {
                node_index, delta, ..
            } => {
                let node_index = *node_index;
                let (content, summary) = match delta {
                    NodeDelta::Reasoning {
                        content, summary, ..
                    } => (content.clone(), summary.clone()),
                    _ => return,
                };
                match self.nodes.remove(&node_index) {
                    Some(NodeMode::Pending(start)) => {
                        if let Some((kind, plain)) =
                            plain_text(content.as_deref(), summary.as_deref())
                        {
                            let id = match &start {
                                UrpStreamEvent::NodeStart {
                                    header: NodeHeader::Reasoning { id, .. },
                                    ..
                                } => id.clone(),
                                _ => None,
                            };
                            self.replacement = Some(vec![
                                UrpStreamEvent::NodeStart {
                                    node_index,
                                    header: text_header(id.clone()),
                                    extra_body: HashMap::new(),
                                },
                                text_delta(node_index, format!("<{tag}>{plain}")),
                            ]);
                            self.nodes.insert(
                                node_index,
                                NodeMode::Converted {
                                    kind,
                                    text: plain,
                                    id,
                                },
                            );
                        } else {
                            self.passed_through.insert(node_index);
                            self.replacement = Some(vec![start, event.clone()]);
                            self.nodes.insert(node_index, NodeMode::PassThrough);
                        }
                    }
                    Some(NodeMode::Converted { kind, mut text, id }) => {
                        let fragment = match kind {
                            PlainKind::Content => content,
                            PlainKind::Summary => summary,
                        }
                        .filter(|fragment| !fragment.is_empty());
                        self.replacement = Some(match fragment {
                            Some(fragment) => {
                                text.push_str(&fragment);
                                vec![text_delta(node_index, fragment)]
                            }
                            None => Vec::new(),
                        });
                        self.nodes
                            .insert(node_index, NodeMode::Converted { kind, text, id });
                    }
                    Some(NodeMode::Closed) => {
                        self.nodes.insert(node_index, NodeMode::Closed);
                        self.replacement = Some(Vec::new());
                    }
                    Some(mode) => {
                        self.nodes.insert(node_index, mode);
                    }
                    None => {}
                }
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                let node_index = *node_index;
                match self.nodes.remove(&node_index) {
                    Some(NodeMode::Converted { text, id, kind }) => {
                        let id = id.or_else(|| node.id().cloned());
                        self.nodes
                            .insert(node_index, NodeMode::Converted { kind, text, id });
                        self.replacement = Some(self.close_node(node_index, tag));
                    }
                    Some(NodeMode::Closed) => {
                        self.nodes.insert(node_index, NodeMode::Closed);
                        self.replacement = Some(Vec::new());
                    }
                    Some(NodeMode::Pending(start)) => match convert_node(node, tag) {
                        Some(converted) => {
                            let Node::Text { content, id, .. } = &converted else {
                                unreachable!("converted reasoning is text");
                            };
                            self.replacement = Some(vec![
                                UrpStreamEvent::NodeStart {
                                    node_index,
                                    header: text_header(id.clone()),
                                    extra_body: HashMap::new(),
                                },
                                text_delta(node_index, content.clone()),
                                UrpStreamEvent::NodeDone {
                                    node_index,
                                    node: converted,
                                    usage: None,
                                    extra_body: HashMap::new(),
                                },
                            ]);
                        }
                        None => {
                            self.passed_through.insert(node_index);
                            self.replacement = Some(vec![start, event.clone()]);
                        }
                    },
                    Some(NodeMode::PassThrough) | None => {}
                }
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                for (position, node) in output.iter_mut().enumerate() {
                    let position = position as u32;
                    if self.passed_through.contains(&position) {
                        continue;
                    }
                    if let Some(closed) = self.closed.remove(&position) {
                        *node = closed;
                        continue;
                    }
                    if let Some(converted) = convert_node(node, tag) {
                        *node = converted;
                    }
                }
            }
            _ => {}
        }
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningToThinkXmlTransform),
});

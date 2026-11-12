use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, NodeHeader, OrdinaryRole, TerminalNode, UrpStreamEvent};
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
    /// `C` emitted for the current segment; later plain text resumes the node (TXO-3).
    Closed(PlainKind),
    /// Later plain text, held until no other stream node is open (TXO-3).
    Resumed {
        kind: PlainKind,
        text: String,
    },
}

#[derive(Default)]
struct StreamState {
    next_index: u32,
    /// Output index of each source node's current segment.
    index_map: HashMap<u32, u32>,
    nodes: HashMap<u32, NodeMode>,
    /// Started source nodes without a `NodeDone` or closed segment.
    open: HashSet<u32>,
    /// Closed output segments of each converted source node.
    segments: HashMap<u32, Vec<(u32, Node)>>,
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
    fn allocate(&mut self) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    fn output_index(&mut self, source: u32) -> u32 {
        if let Some(out) = self.index_map.get(&source) {
            return *out;
        }
        let out = self.allocate();
        self.index_map.insert(source, out);
        out
    }

    /// TXO-3: closes every open converted node except `keep`, in output order.
    fn close_open_nodes(&mut self, keep: Option<u32>, tag: &str) -> Vec<UrpStreamEvent> {
        let mut open: Vec<(u32, u32)> = self
            .nodes
            .iter()
            .filter(|(source, mode)| {
                Some(**source) != keep && matches!(mode, NodeMode::Converted { .. })
            })
            .map(|(source, _)| (self.index_map[source], *source))
            .collect();
        open.sort_unstable();
        let mut out = Vec::with_capacity(open.len() * 2);
        for (_, source) in open {
            out.extend(self.close_node(source, tag));
        }
        out
    }

    fn close_node(&mut self, source: u32, tag: &str) -> Vec<UrpStreamEvent> {
        let Some(NodeMode::Converted { kind, text, id }) = self.nodes.remove(&source) else {
            return Vec::new();
        };
        self.nodes.insert(source, NodeMode::Closed(kind));
        self.open.remove(&source);
        let node_index = self.index_map[&source];
        let node = text_node(id, format!("<{tag}>{text}</{tag}>"));
        self.segments
            .entry(source)
            .or_default()
            .push((node_index, node.clone()));
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

    /// Opens a converted text segment at `node_index` with its first plain fragment.
    fn open_segment(
        &mut self,
        source: u32,
        node_index: u32,
        kind: PlainKind,
        plain: String,
        id: Option<String>,
        tag: &str,
    ) -> Vec<UrpStreamEvent> {
        let events = vec![
            UrpStreamEvent::NodeStart {
                node_index,
                header: text_header(id.clone()),
                extra_body: HashMap::new(),
            },
            text_delta(node_index, format!("<{tag}>{plain}")),
        ];
        self.nodes.insert(
            source,
            NodeMode::Converted {
                kind,
                text: plain,
                id,
            },
        );
        events
    }

    /// Emits held resumed text as complete text segments once `force` is set or no other
    /// stream node is open, so node completion stays in output-index order (TXO-3).
    fn flush_resumed(&mut self, force: bool, tag: &str) -> Vec<UrpStreamEvent> {
        if !force && !self.open.is_empty() {
            return Vec::new();
        }
        let mut sources: Vec<u32> = self
            .nodes
            .iter()
            .filter(|(_, mode)| matches!(mode, NodeMode::Resumed { .. }))
            .map(|(source, _)| *source)
            .collect();
        sources.sort_unstable();
        let mut out = Vec::with_capacity(sources.len() * 3);
        for source in sources {
            let Some(NodeMode::Resumed { kind, text }) = self.nodes.remove(&source) else {
                continue;
            };
            self.nodes.insert(source, NodeMode::Closed(kind));
            let node_index = self.allocate();
            self.index_map.insert(source, node_index);
            let content = format!("<{tag}>{text}</{tag}>");
            let node = text_node(None, content.clone());
            self.segments
                .entry(source)
                .or_default()
                .push((node_index, node.clone()));
            out.extend([
                UrpStreamEvent::NodeStart {
                    node_index,
                    header: text_header(None),
                    extra_body: HashMap::new(),
                },
                text_delta(node_index, content),
                UrpStreamEvent::NodeDone {
                    node_index,
                    node,
                    usage: None,
                    extra_body: HashMap::new(),
                },
            ]);
        }
        out
    }

    /// STR-8c: each entry takes the output index its source node was streamed under. A converted
    /// source expands into its streamed segments. Streamed entries keep output-index order and
    /// precede the entries the stream never announced.
    fn convert_terminal_output(
        &mut self,
        output: Vec<TerminalNode>,
        tag: &str,
    ) -> Vec<TerminalNode> {
        let mut streamed: Vec<(u32, TerminalNode)> = Vec::with_capacity(output.len());
        let mut unannounced = Vec::new();
        for TerminalNode { node_index, node } in output {
            let source = node_index
                .and_then(|source| self.index_map.remove(&source).map(|out| (source, out)));
            let Some((source, out)) = source else {
                let node = convert_node(&node, tag).unwrap_or(node);
                unannounced.push(TerminalNode::unannounced(node));
                continue;
            };
            if let Some(segments) = self.segments.remove(&source) {
                streamed.extend(
                    segments
                        .into_iter()
                        .map(|(out, node)| (out, TerminalNode::streamed(out, node))),
                );
                continue;
            }
            let node = if self.passed_through.contains(&source) {
                node
            } else {
                convert_node(&node, tag).unwrap_or(node)
            };
            // A reasoning node still pending never had its `NodeStart` forwarded.
            if matches!(self.nodes.get(&source), Some(NodeMode::Pending(_))) {
                unannounced.push(TerminalNode::unannounced(node));
            } else {
                streamed.push((out, TerminalNode::streamed(out, node)));
            }
        }
        streamed.sort_by_key(|(out, _)| *out);
        streamed
            .into_iter()
            .map(|(_, entry)| entry)
            .chain(unannounced)
            .collect()
    }

    fn apply(&mut self, event: &mut UrpStreamEvent, tag: &str) {
        let is_terminal = matches!(event, UrpStreamEvent::ResponseDone { .. });
        let mut closing = match event {
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
        if is_terminal {
            closing.extend(self.flush_resumed(true, tag));
        }
        self.convert_event(event, tag);
        let resumed = if is_terminal {
            Vec::new()
        } else {
            self.flush_resumed(false, tag)
        };
        if !closing.is_empty() || !resumed.is_empty() {
            let rest = self
                .replacement
                .take()
                .unwrap_or_else(|| vec![event.clone()]);
            self.replacement = Some(closing.into_iter().chain(rest).chain(resumed).collect());
        }
    }

    fn convert_event(&mut self, event: &mut UrpStreamEvent, tag: &str) {
        match event {
            UrpStreamEvent::NodeStart {
                node_index, header, ..
            } => {
                let source = *node_index;
                let is_reasoning = matches!(header, NodeHeader::Reasoning { .. });
                if !matches!(header, NodeHeader::NextDownstreamEnvelopeExtra) {
                    self.open.insert(source);
                }
                *node_index = self.output_index(source);
                if is_reasoning {
                    self.nodes.insert(source, NodeMode::Pending(event.clone()));
                    self.replacement = Some(Vec::new());
                }
            }
            UrpStreamEvent::NodeDelta {
                node_index, delta, ..
            } => {
                let source = *node_index;
                let out = self.output_index(source);
                *node_index = out;
                let (content, summary) = match delta {
                    NodeDelta::Reasoning {
                        content, summary, ..
                    } => (content.clone(), summary.clone()),
                    _ => return,
                };
                match self.nodes.remove(&source) {
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
                            self.replacement =
                                Some(self.open_segment(source, out, kind, plain, id, tag));
                        } else {
                            self.passed_through.insert(source);
                            self.replacement = Some(vec![start, event.clone()]);
                            self.nodes.insert(source, NodeMode::PassThrough);
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
                                vec![text_delta(out, fragment)]
                            }
                            None => Vec::new(),
                        });
                        self.nodes
                            .insert(source, NodeMode::Converted { kind, text, id });
                    }
                    Some(NodeMode::Closed(kind)) => {
                        let fragment = match kind {
                            PlainKind::Content => content,
                            PlainKind::Summary => summary,
                        }
                        .filter(|fragment| !fragment.is_empty());
                        self.nodes.insert(
                            source,
                            match fragment {
                                Some(text) => NodeMode::Resumed { kind, text },
                                None => NodeMode::Closed(kind),
                            },
                        );
                        self.replacement = Some(Vec::new());
                    }
                    Some(NodeMode::Resumed { kind, mut text }) => {
                        let fragment = match kind {
                            PlainKind::Content => content,
                            PlainKind::Summary => summary,
                        };
                        text.push_str(fragment.as_deref().unwrap_or_default());
                        self.nodes.insert(source, NodeMode::Resumed { kind, text });
                        self.replacement = Some(Vec::new());
                    }
                    Some(mode) => {
                        self.nodes.insert(source, mode);
                    }
                    None => {}
                }
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                let source = *node_index;
                let out = self.output_index(source);
                *node_index = out;
                self.open.remove(&source);
                match self.nodes.remove(&source) {
                    Some(NodeMode::Converted { kind, text, id }) => {
                        let id = id.or_else(|| {
                            (!self.segments.contains_key(&source))
                                .then(|| node.id().cloned())
                                .flatten()
                        });
                        self.nodes
                            .insert(source, NodeMode::Converted { kind, text, id });
                        self.replacement = Some(self.close_node(source, tag));
                    }
                    Some(mode @ (NodeMode::Closed(_) | NodeMode::Resumed { .. })) => {
                        self.nodes.insert(source, mode);
                        self.replacement = Some(Vec::new());
                    }
                    Some(NodeMode::Pending(start)) => match convert_node(node, tag) {
                        Some(converted) => {
                            let Node::Text { content, id, .. } = &converted else {
                                unreachable!("converted reasoning is text");
                            };
                            self.replacement = Some(vec![
                                UrpStreamEvent::NodeStart {
                                    node_index: out,
                                    header: text_header(id.clone()),
                                    extra_body: HashMap::new(),
                                },
                                text_delta(out, content.clone()),
                                UrpStreamEvent::NodeDone {
                                    node_index: out,
                                    node: converted.clone(),
                                    usage: None,
                                    extra_body: HashMap::new(),
                                },
                            ]);
                            self.segments
                                .entry(source)
                                .or_default()
                                .push((out, converted));
                        }
                        None => {
                            self.passed_through.insert(source);
                            self.replacement = Some(vec![start, event.clone()]);
                        }
                    },
                    Some(NodeMode::PassThrough) | None => {}
                }
            }
            UrpStreamEvent::ProviderControl {
                event_name, data, ..
            } => {
                // TXO-5: late citations follow their text node's output index.
                if let Some((source, citations)) =
                    crate::urp::citations::parse_chat_annotations_control(event_name, data)
                {
                    let out = self.output_index(source);
                    *event = crate::urp::citations::chat_annotations_control(out, &citations);
                }
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                *output = self.convert_terminal_output(std::mem::take(output), tag);
            }
            _ => {}
        }
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningToThinkXmlTransform),
});

use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::citations::{Citation, TextSpan, rebase_citation};
use crate::urp::{Node, NodeDelta, NodeHeader, OrdinaryRole, TerminalNode, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Config {
    tag: String,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Reasoning,
}

struct OpenSegment {
    out_index: u32,
    mode: Mode,
    content: String,
    source_start: u64,
    citations: Vec<Citation>,
}

struct TextSplit {
    phase: Option<String>,
    id: Option<String>,
    mode: Mode,
    buffer: String,
    open: Option<OpenSegment>,
    /// Source characters consumed so far, including tags (TXI-6).
    source_pos: u64,
    /// Kept runs of the closed text segments.
    spans: Vec<TextSpan>,
    /// Citations whose range is not segmented yet.
    pending: Vec<Citation>,
    /// Citations already received through deltas.
    seen_citations: usize,
}

/// TXI-3..TXI-5 state: per-text-node split state plus dense output index assignment.
#[derive(Default)]
struct StreamState {
    next_index: u32,
    index_map: HashMap<u32, u32>,
    text_nodes: HashMap<u32, TextSplit>,
    split_done: HashMap<u32, Vec<(u32, Node)>>,
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

pub struct ReasoningFromThinkXmlTransform;

#[async_trait]
impl Transform for ReasoningFromThinkXmlTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_from_think_xml"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: from think XML"),
            ("zh", "推理：解析 think XML"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Parses the configured think-tag XML out of assistant text and restores it as reasoning nodes. Inverse of reasoning_to_think_xml.",
            ),
            (
                "zh",
                "从 assistant 文本中解析配置的 think 标签并还原为推理节点。与 reasoning_to_think_xml 互逆。",
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
                let mut out = Vec::with_capacity(resp.output.len());
                for node in resp.output.drain(..) {
                    match node {
                        Node::Text {
                            role: OrdinaryRole::Assistant,
                            content,
                            phase,
                            citations,
                            ..
                        } => {
                            out.extend(extract_text_and_reasoning(
                                &content, phase, citations, &cfg.tag,
                            ));
                        }
                        other => out.push(other),
                    }
                }
                resp.output = out;
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<StreamState>() else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                apply_stream(event, stream_state, &cfg.tag);
            }
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn extract_text_and_reasoning(
    content: &str,
    phase: Option<String>,
    source_citations: Vec<Citation>,
    tag: &str,
) -> Vec<Node> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let chars = |text: &str| text.chars().count() as u64;
    let mut nodes = Vec::new();
    let mut spans = Vec::new();
    let mut rest = content;
    let mut source_pos = 0u64;
    let mut push_text = |nodes: &mut Vec<Node>, text: &str, source_pos: u64| {
        if !text.is_empty() {
            spans.push(TextSpan {
                group: nodes.len() as u32,
                source_start: source_pos,
                source_end: source_pos + chars(text),
                out_start: 0,
            });
            nodes.push(text_node(text.to_string(), phase.clone(), None));
        }
    };

    loop {
        let Some(start) = rest.find(&open) else {
            push_text(&mut nodes, rest, source_pos);
            break;
        };
        let before = &rest[..start];
        push_text(&mut nodes, before, source_pos);
        source_pos += chars(before) + chars(&open);
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(&close) else {
            if !after_open.is_empty() {
                nodes.push(reasoning_node(after_open.to_string()));
            }
            break;
        };
        let reasoning = &after_open[..end];
        if !reasoning.is_empty() {
            nodes.push(reasoning_node(reasoning.to_string()));
        }
        source_pos += chars(reasoning) + chars(&close);
        rest = &after_open[end + close.len()..];
    }
    for mut citation in source_citations {
        if let Some(group) = rebase_citation(&mut citation, &spans)
            && let Some(Node::Text { citations, .. }) = nodes.get_mut(group as usize)
        {
            citations.push(citation);
        }
    }
    nodes
}

fn text_node(content: String, phase: Option<String>, id: Option<String>) -> Node {
    Node::Text {
        logprobs: None,
        signature: None,
        citations: Vec::new(),
        id,
        role: OrdinaryRole::Assistant,
        content,
        phase,
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }
}

fn reasoning_node(content: String) -> Node {
    Node::Reasoning {
        metadata: Default::default(),
        id: None,
        content: Some(content),
        encrypted: None,
        summary: None,
        source: None,
        extra_body: HashMap::new(),
    }
}

/// Bytes at the end of `buffer` that may begin `tag` once more text arrives.
fn held_tag_prefix_len(buffer: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&len| tag.is_char_boundary(len) && buffer.ends_with(&tag[..len]))
        .unwrap_or(0)
}

impl StreamState {
    fn allocate(&mut self) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    fn close_segment(&mut self, source: u32, out: &mut Vec<UrpStreamEvent>) {
        let Some(split) = self.text_nodes.get_mut(&source) else {
            return;
        };
        let Some(segment) = split.open.take() else {
            return;
        };
        let node = match segment.mode {
            Mode::Text => {
                split.spans.push(TextSpan {
                    group: segment.out_index,
                    source_start: segment.source_start,
                    source_end: split.source_pos,
                    out_start: 0,
                });
                let id = split.id.take();
                let mut node = text_node(segment.content, split.phase.clone(), id);
                if let Node::Text { citations, .. } = &mut node {
                    *citations = segment.citations;
                }
                node
            }
            Mode::Reasoning => reasoning_node(segment.content),
        };
        self.split_done
            .entry(source)
            .or_default()
            .push((segment.out_index, node.clone()));
        out.push(UrpStreamEvent::NodeDone {
            node_index: segment.out_index,
            node,
            usage: None,
            extra_body: HashMap::new(),
        });
    }

    fn emit_segment(&mut self, source: u32, text: &str, out: &mut Vec<UrpStreamEvent>) {
        if text.is_empty() {
            return;
        }
        let mode = self.text_nodes[&source].mode;
        if self.text_nodes[&source].open.is_none() {
            let out_index = self.allocate();
            let split = self.text_nodes.get_mut(&source).expect("text split state");
            let header = match mode {
                Mode::Text => NodeHeader::Text {
                    signature: None,
                    citations: Vec::new(),
                    id: split.id.clone(),
                    role: OrdinaryRole::Assistant,
                    phase: split.phase.clone(),
                },
                Mode::Reasoning => NodeHeader::Reasoning {
                    metadata: Default::default(),
                    id: None,
                },
            };
            split.open = Some(OpenSegment {
                out_index,
                mode,
                content: String::new(),
                source_start: split.source_pos,
                citations: Vec::new(),
            });
            out.push(UrpStreamEvent::NodeStart {
                node_index: out_index,
                header,
                extra_body: HashMap::new(),
            });
        }
        let split = self.text_nodes.get_mut(&source).expect("text split state");
        split.source_pos += text.chars().count() as u64;
        let segment = split.open.as_mut().expect("open segment");
        segment.content.push_str(text);
        let delta = match mode {
            Mode::Text => NodeDelta::Text {
                logprobs: None,
                signature: None,
                citations: Vec::new(),
                content: text.to_string(),
            },
            Mode::Reasoning => NodeDelta::Reasoning {
                metadata: Default::default(),
                content: Some(text.to_string()),
                encrypted: None,
                summary: None,
                source: None,
            },
        };
        out.push(UrpStreamEvent::NodeDelta {
            node_index: segment.out_index,
            delta,
            usage: None,
            extra_body: HashMap::new(),
        });
    }

    /// Emits every buffered byte that cannot start the awaited tag (TXI-3).
    fn drain(
        &mut self,
        source: u32,
        open: &str,
        close: &str,
        terminal: bool,
    ) -> Vec<UrpStreamEvent> {
        let mut out = Vec::new();
        loop {
            let split = &self.text_nodes[&source];
            let tag = match split.mode {
                Mode::Text => open,
                Mode::Reasoning => close,
            };
            if let Some(position) = split.buffer.find(tag) {
                let before = split.buffer[..position].to_string();
                self.emit_segment(source, &before, &mut out);
                self.close_segment(source, &mut out);
                let split = self.text_nodes.get_mut(&source).expect("text split state");
                split.buffer.drain(..position + tag.len());
                split.source_pos += tag.chars().count() as u64;
                split.mode = match split.mode {
                    Mode::Text => Mode::Reasoning,
                    Mode::Reasoning => Mode::Text,
                };
                continue;
            }
            let held = if terminal {
                0
            } else {
                held_tag_prefix_len(&split.buffer, tag)
            };
            let ready = split.buffer.len() - held;
            let text = split.buffer[..ready].to_string();
            self.emit_segment(source, &text, &mut out);
            self.text_nodes
                .get_mut(&source)
                .expect("text split state")
                .buffer
                .drain(..ready);
            break;
        }
        if terminal {
            self.close_segment(source, &mut out);
        }
        out
    }

    /// TXI-6: moves held citations to their segments. A citation of a completed segment joins
    /// that segment's terminal node and goes to Chat downstreams as a control event.
    fn place_citations(&mut self, source: u32, terminal: bool, out: &mut Vec<UrpStreamEvent>) {
        let Some(split) = self.text_nodes.get_mut(&source) else {
            return;
        };
        let mut spans = split.spans.clone();
        if let Some(open) = split.open.as_ref().filter(|open| open.mode == Mode::Text) {
            spans.push(TextSpan {
                group: open.out_index,
                source_start: open.source_start,
                source_end: split.source_pos,
                out_start: 0,
            });
        }
        for mut citation in std::mem::take(&mut split.pending) {
            let ready = terminal
                || match &citation.answer_range {
                    Some(range) => spans.iter().any(|span| span.source_end > range.start),
                    None => !spans.is_empty(),
                };
            if !ready {
                split.pending.push(citation);
                continue;
            }
            // No text segment remains to cite.
            let Some(group) = rebase_citation(&mut citation, &spans) else {
                continue;
            };
            if let Some(open) = split
                .open
                .as_mut()
                .filter(|open| open.mode == Mode::Text && open.out_index == group)
            {
                open.citations.push(citation.clone());
                out.push(UrpStreamEvent::NodeDelta {
                    node_index: group,
                    delta: NodeDelta::Text {
                        logprobs: None,
                        signature: None,
                        citations: vec![citation],
                        content: String::new(),
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                });
                continue;
            }
            if let Some((_, Node::Text { citations, .. })) = self
                .split_done
                .get_mut(&source)
                .and_then(|segments| segments.iter_mut().find(|(index, _)| *index == group))
            {
                citations.push(citation.clone());
            }
            out.push(crate::urp::citations::chat_annotations_control(
                group,
                &[citation],
            ));
        }
    }

    /// STR-8c: a split text entry expands into the segments streamed for it, and every other
    /// streamed entry takes its remapped index. Streamed entries keep output-index order and
    /// precede the entries the stream never announced, which are split here.
    fn convert_terminal_output(
        &mut self,
        output: Vec<TerminalNode>,
        tag: &str,
    ) -> Vec<TerminalNode> {
        let mut streamed: Vec<(u32, TerminalNode)> = Vec::with_capacity(output.len());
        let mut unannounced = Vec::new();
        for TerminalNode { node_index, node } in output {
            if let Some(source) = node_index {
                if self.text_nodes.remove(&source).is_some() {
                    // A streamed text node that produced only empty segments has no entry.
                    let segments = self.split_done.remove(&source).unwrap_or_default();
                    streamed.extend(
                        segments
                            .into_iter()
                            .map(|(out, node)| (out, TerminalNode::streamed(out, node))),
                    );
                    continue;
                }
                if let Some(out) = self.index_map.remove(&source) {
                    streamed.push((out, TerminalNode::streamed(out, node)));
                    continue;
                }
            }
            match node {
                Node::Text {
                    role: OrdinaryRole::Assistant,
                    content,
                    phase,
                    citations,
                    ..
                } => unannounced.extend(
                    extract_text_and_reasoning(&content, phase, citations, tag)
                        .into_iter()
                        .map(TerminalNode::unannounced),
                ),
                other => unannounced.push(TerminalNode::unannounced(other)),
            }
        }
        streamed.sort_by_key(|(out, _)| *out);
        streamed
            .into_iter()
            .map(|(_, entry)| entry)
            .chain(unannounced)
            .collect()
    }

    fn remap(&mut self, node_index: &mut u32) {
        if let Some(out) = self.index_map.get(node_index) {
            *node_index = *out;
        }
    }
}

fn apply_stream(event: &mut UrpStreamEvent, state: &mut StreamState, tag: &str) {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    match event {
        UrpStreamEvent::NodeStart {
            node_index,
            header:
                NodeHeader::Text {
                    role: OrdinaryRole::Assistant,
                    id,
                    phase,
                    ..
                },
            ..
        } => {
            state.text_nodes.insert(
                *node_index,
                TextSplit {
                    phase: phase.clone(),
                    id: id.clone(),
                    mode: Mode::Text,
                    buffer: String::new(),
                    open: None,
                    source_pos: 0,
                    spans: Vec::new(),
                    pending: Vec::new(),
                    seen_citations: 0,
                },
            );
            state.replacement = Some(Vec::new());
        }
        UrpStreamEvent::NodeStart { node_index, .. } => {
            let out = state.allocate();
            state.index_map.insert(*node_index, out);
            *node_index = out;
        }
        UrpStreamEvent::NodeDelta {
            node_index, delta, ..
        } if state.text_nodes.contains_key(node_index) => {
            let source = *node_index;
            let mut out = Vec::new();
            if let NodeDelta::Text {
                content, citations, ..
            } = delta
            {
                let split = state.text_nodes.get_mut(&source).expect("text split state");
                split.buffer.push_str(content);
                split.seen_citations += citations.len();
                split.pending.append(citations);
                out = state.drain(source, &open, &close, false);
                state.place_citations(source, false, &mut out);
            }
            state.replacement = Some(out);
        }
        UrpStreamEvent::NodeDelta { node_index, .. } => state.remap(node_index),
        UrpStreamEvent::NodeDone {
            node_index, node, ..
        } if state.text_nodes.contains_key(node_index) => {
            let source = *node_index;
            let split = state.text_nodes.get_mut(&source).expect("text split state");
            if let Node::Text { citations, .. } = node {
                let seen = split.seen_citations.min(citations.len());
                split.pending.extend(citations.drain(seen..));
            }
            let mut out = Vec::new();
            state.place_citations(source, false, &mut out);
            out.extend(state.drain(source, &open, &close, true));
            state.place_citations(source, true, &mut out);
            state.replacement = Some(out);
        }
        UrpStreamEvent::NodeDone { node_index, .. } => state.remap(node_index),
        UrpStreamEvent::ProviderControl {
            event_name, data, ..
        } => {
            if let Some((source, citations)) =
                crate::urp::citations::parse_chat_annotations_control(event_name, data)
            {
                if let Some(split) = state.text_nodes.get_mut(&source) {
                    split.pending.extend(citations);
                    let mut out = Vec::new();
                    state.place_citations(source, true, &mut out);
                    state.replacement = Some(out);
                } else if let Some(mapped) = state.index_map.get(&source) {
                    *event = crate::urp::citations::chat_annotations_control(*mapped, &citations);
                }
            }
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            *output = state.convert_terminal_output(std::mem::take(output), tag);
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningFromThinkXmlTransform),
});

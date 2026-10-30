use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::citations::{Citation, TextSpan, rebase_citation};
use crate::urp::{ImageSource, Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ImageMarkdownToOutputTransform;

struct StreamTextNodeState {
    header_id: Option<String>,
    header_phase: Option<String>,
    header_extra_body: HashMap<String, Value>,
    buffered_tail: String,
    cleaned_content: String,
    start_emitted: bool,
    saw_delta: bool,
    /// Source characters consumed so far, including removed image markdown (AMIO-13).
    source_chars: u64,
    spans: Vec<TextSpan>,
    citations: Vec<Citation>,
    pending_citations: Vec<Citation>,
    seen_citations: usize,
}

struct StreamState {
    replacement: Option<Vec<UrpStreamEvent>>,
    node_text_parts: HashMap<u32, StreamTextNodeState>,
    /// Kept runs of completed text nodes, for late citations.
    done_spans: HashMap<u32, Vec<TextSpan>>,
    next_synthetic_node_index: u32,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            replacement: None,
            node_text_parts: HashMap::new(),
            done_spans: HashMap::new(),
            next_synthetic_node_index: u32::MAX,
        }
    }
}

impl TransformState for StreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn finalize_stream_event(&mut self, event: UrpStreamEvent) -> Vec<UrpStreamEvent> {
        self.replacement.take().unwrap_or_else(|| vec![event])
    }
}

#[async_trait]
impl Transform for ImageMarkdownToOutputTransform {
    fn type_id(&self) -> &'static str {
        "image_markdown_to_output"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Image: Markdown to image nodes"),
            ("zh", "图像：Markdown 转图像节点"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Extracts Markdown image blocks from assistant text and converts them into ordinary assistant image nodes. Inverse of image_output_to_markdown.",
            ),
            (
                "zh",
                "从 assistant 文本中提取 Markdown 图片语法并转换为标准的 assistant 图像节点。与 image_output_to_markdown 互逆。",
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
            "properties": {},
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
        _config: &dyn TransformConfig,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        match data {
            UrpData::Response(resp) => {
                rewrite_assistant_markdown_images_nodes(&mut resp.output);
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<StreamState>() else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                apply_stream(event, stream_state);
            }
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn rewrite_assistant_markdown_images_nodes(nodes: &mut Vec<Node>) {
    let mut rewritten = Vec::with_capacity(nodes.len());
    for node in nodes.drain(..) {
        match node {
            Node::Text {
                logprobs: _,
                signature: _,
                citations,
                id,
                role: OrdinaryRole::Assistant,
                content,
                phase,
                extra_body,
                cache_breakpoint,
            } => {
                let (cleaned, images, spans) = extract_markdown_images_from_text(&content);
                if !cleaned.is_empty() {
                    let mut citations = citations;
                    for citation in &mut citations {
                        rebase_citation(citation, &spans);
                    }
                    rewritten.push(Node::Text {
                        logprobs: None,
                        signature: None,
                        citations,
                        id,
                        role: OrdinaryRole::Assistant,
                        content: cleaned,
                        phase,
                        extra_body,
                        cache_breakpoint,
                    });
                }
                for source in images {
                    rewritten.push(Node::Image {
                        metadata: Default::default(),

                        id: None,
                        role: OrdinaryRole::Assistant,
                        source,
                        extra_body: HashMap::new(),
                    });
                }
            }
            other => rewritten.push(other),
        }
    }
    *nodes = rewritten;
}

fn extract_markdown_images_from_text(content: &str) -> (String, Vec<ImageSource>, Vec<TextSpan>) {
    let (segments, tail) = split_stream_segments(content, true);
    debug_assert!(tail.is_empty());
    let mut images = Vec::new();
    let mut cleaned = String::new();
    let mut spans = Vec::new();
    for segment in segments {
        match segment {
            StreamSegment::Text {
                content: text,
                source_start,
            } => {
                spans.push(kept_span(content, 0, source_start, &text, &cleaned));
                cleaned.push_str(&text);
            }
            StreamSegment::Image(source) => images.push(source),
        }
    }
    (cleaned, images, spans)
}

/// The run `text`, found at byte `source_start` of `source`, whose first character has character
/// position `source_base` plus its position in `source`.
fn kept_span(
    source: &str,
    source_base: u64,
    source_start: usize,
    text: &str,
    cleaned: &str,
) -> TextSpan {
    let start = source_base + source[..source_start].chars().count() as u64;
    TextSpan {
        group: 0,
        source_start: start,
        source_end: start + text.chars().count() as u64,
        out_start: cleaned.chars().count() as u64,
    }
}

#[derive(Debug)]
enum StreamSegment {
    Text { content: String, source_start: usize },
    Image(ImageSource),
}

enum CandidateParse {
    Valid { end: usize, source: ImageSource },
    Invalid { safe_end: usize },
    Incomplete,
}

fn split_stream_segments(content: &str, terminal: bool) -> (Vec<StreamSegment>, String) {
    let bytes = content.as_bytes();
    let mut i = 0usize;
    let mut text = String::new();
    let mut text_start = 0usize;
    let mut segments = Vec::new();

    while i < bytes.len() {
        if bytes[i] == b'!' && bytes.get(i + 1) == Some(&b'[') {
            match parse_markdown_candidate(content, i) {
                CandidateParse::Valid { end, source } => {
                    if !text.is_empty() {
                        segments.push(StreamSegment::Text {
                            content: std::mem::take(&mut text),
                            source_start: text_start,
                        });
                    }
                    segments.push(StreamSegment::Image(source));
                    i = end;
                    continue;
                }
                CandidateParse::Invalid { safe_end } => {
                    if text.is_empty() {
                        text_start = i;
                    }
                    text.push_str(&content[i..safe_end]);
                    i = safe_end;
                    continue;
                }
                CandidateParse::Incomplete => break,
            }
        }

        let ch = content[i..].chars().next().expect("stream parser char");
        if text.is_empty() {
            text_start = i;
        }
        text.push(ch);
        i += ch.len_utf8();
    }

    let mut tail = if i < bytes.len() {
        content[i..].to_string()
    } else {
        String::new()
    };
    if terminal && !tail.is_empty() {
        if text.is_empty() {
            text_start = i;
        }
        text.push_str(&tail);
        tail.clear();
    }
    if !text.is_empty() {
        segments.push(StreamSegment::Text {
            content: text,
            source_start: text_start,
        });
    }
    (segments, tail)
}

fn parse_markdown_candidate(content: &str, start: usize) -> CandidateParse {
    let bytes = content.as_bytes();
    let mut close_bracket = start + 2;
    while close_bracket < bytes.len() && bytes[close_bracket] != b']' {
        close_bracket += 1;
    }
    if close_bracket >= bytes.len() {
        return CandidateParse::Incomplete;
    }
    let after_bracket = close_bracket + 1;
    if after_bracket >= bytes.len() {
        return CandidateParse::Incomplete;
    }
    if bytes[after_bracket] != b'(' {
        return CandidateParse::Invalid {
            safe_end: after_bracket,
        };
    }

    let url_start = after_bracket + 1;
    if url_start >= bytes.len() {
        return CandidateParse::Incomplete;
    }

    let mut i = url_start;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b')' {
            if i == url_start {
                return CandidateParse::Invalid { safe_end: i + 1 };
            }
            let url = &content[url_start..i];
            return match parse_markdown_image_source(url) {
                Some(source) => CandidateParse::Valid { end: i + 1, source },
                None => CandidateParse::Invalid { safe_end: i + 1 },
            };
        }
        if byte.is_ascii_whitespace() {
            return CandidateParse::Invalid { safe_end: i + 1 };
        }
        i += 1;
    }
    CandidateParse::Incomplete
}

fn apply_stream(event: &mut UrpStreamEvent, state: &mut StreamState) {
    if matches!(
        event,
        UrpStreamEvent::NodeStart {
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            ..
        } | UrpStreamEvent::NodeDone {
            node: Node::NextDownstreamEnvelopeExtra { .. },
            ..
        }
    ) {
        state.replacement = Some(vec![event.clone()]);
        return;
    }
    if let UrpStreamEvent::ProviderControl {
        event_name, data, ..
    } = event
    {
        // AMIO-13: late citations of a completed node follow its kept text.
        if let Some((node_index, mut citations)) =
            crate::urp::citations::parse_chat_annotations_control(event_name, data)
            && let Some(spans) = state.done_spans.get(&node_index)
        {
            for citation in &mut citations {
                rebase_citation(citation, spans);
            }
            *event = crate::urp::citations::chat_annotations_control(node_index, &citations);
        }
        return;
    }
    if apply_node_stream(event, state) {
        return;
    }
    if let UrpStreamEvent::ResponseDone { output, .. } = event {
        rewrite_assistant_markdown_images_nodes(output);
        state.node_text_parts.clear();
    }
}

fn apply_node_stream(event: &mut UrpStreamEvent, state: &mut StreamState) -> bool {
    match event {
        UrpStreamEvent::NodeStart {
            node_index,
            header:
                NodeHeader::Text {
                    signature: _,
                    citations: _,
                    id,
                    role: OrdinaryRole::Assistant,
                    phase,
                },
            extra_body,
        } => {
            state.node_text_parts.insert(
                *node_index,
                StreamTextNodeState {
                    header_id: id.clone(),
                    header_phase: phase.clone(),
                    header_extra_body: extra_body.clone(),
                    buffered_tail: String::new(),
                    cleaned_content: String::new(),
                    start_emitted: false,
                    saw_delta: false,
                    source_chars: 0,
                    spans: Vec::new(),
                    citations: Vec::new(),
                    pending_citations: Vec::new(),
                    seen_citations: 0,
                },
            );
            state.replacement = Some(Vec::new());
            true
        }
        UrpStreamEvent::NodeDelta {
            node_index,
            delta:
                NodeDelta::Text {
                    logprobs: _,
                    signature: _,
                    citations,
                    content,
                },
            usage,
            extra_body,
        } => {
            let Some(mut node_state) = state.node_text_parts.remove(node_index) else {
                return false;
            };
            node_state.saw_delta = true;
            let combined = format!("{}{}", node_state.buffered_tail, content);
            let (segments, tail) = split_stream_segments(&combined, false);
            node_state.buffered_tail = tail;
            let mut emitted = Vec::new();
            emit_node_segments(
                *node_index,
                state,
                &mut node_state,
                &combined,
                segments,
                &mut emitted,
                extra_body,
            );
            node_state.seen_citations += citations.len();
            node_state.pending_citations.append(citations);
            place_citations(*node_index, &mut node_state, false, &mut emitted);
            attach_usage_to_last_node_event(&mut emitted, usage.clone());
            state.node_text_parts.insert(*node_index, node_state);
            state.replacement = Some(emitted);
            true
        }
        UrpStreamEvent::NodeDone {
            node_index,
            node:
                Node::Text {
                    logprobs: _,
                    signature: _,
                    citations,
                    id,
                    role: OrdinaryRole::Assistant,
                    content,
                    phase,
                    extra_body: node_extra_body,
                    cache_breakpoint: _,
                },
            usage,
            extra_body: event_extra_body,
        } => {
            let mut node_state =
                state
                    .node_text_parts
                    .remove(node_index)
                    .unwrap_or_else(|| StreamTextNodeState {
                        header_id: id.clone(),
                        header_phase: phase.clone(),
                        header_extra_body: HashMap::new(),
                        buffered_tail: String::new(),
                        cleaned_content: String::new(),
                        start_emitted: false,
                        saw_delta: false,
                        source_chars: 0,
                        spans: Vec::new(),
                        citations: Vec::new(),
                        pending_citations: Vec::new(),
                        seen_citations: 0,
                    });
            if !node_state.saw_delta {
                node_state.buffered_tail.push_str(content);
            }
            let source = std::mem::take(&mut node_state.buffered_tail);
            let (segments, tail) = split_stream_segments(&source, true);
            node_state.buffered_tail = tail;
            let mut emitted = Vec::new();
            emit_node_segments(
                *node_index,
                state,
                &mut node_state,
                &source,
                segments,
                &mut emitted,
                &HashMap::new(),
            );
            let seen = node_state.seen_citations.min(citations.len());
            node_state.pending_citations.extend(citations.drain(seen..));
            place_citations(*node_index, &mut node_state, true, &mut emitted);
            state
                .done_spans
                .insert(*node_index, std::mem::take(&mut node_state.spans));
            if node_state.start_emitted {
                emitted.push(UrpStreamEvent::NodeDone {
                    node_index: *node_index,
                    node: Node::Text {
                        logprobs: None,
                        signature: None,
                        citations: std::mem::take(&mut node_state.citations),
                        id: id.clone(),
                        role: OrdinaryRole::Assistant,
                        content: std::mem::take(&mut node_state.cleaned_content),
                        phase: phase.clone(),
                        extra_body: node_extra_body.clone(),
                        cache_breakpoint: None,
                    },
                    usage: None,
                    extra_body: event_extra_body.clone(),
                });
            }
            attach_usage_to_last_node_event(&mut emitted, usage.clone());
            state.replacement = Some(emitted);
            true
        }
        _ => false,
    }
}

fn allocate_synthetic_node_index(state: &mut StreamState) -> u32 {
    let node_index = state.next_synthetic_node_index;
    state.next_synthetic_node_index = state.next_synthetic_node_index.saturating_sub(1);
    node_index
}

fn ensure_text_node_start(
    node_index: u32,
    node_state: &mut StreamTextNodeState,
    emitted: &mut Vec<UrpStreamEvent>,
) {
    if node_state.start_emitted {
        return;
    }
    emitted.push(UrpStreamEvent::NodeStart {
        node_index,
        header: NodeHeader::Text {
            signature: None,
            citations: Vec::new(),
            id: node_state.header_id.clone(),
            role: OrdinaryRole::Assistant,
            phase: node_state.header_phase.clone(),
        },
        extra_body: node_state.header_extra_body.clone(),
    });
    node_state.start_emitted = true;
}

fn emit_synthetic_image_node(
    state: &mut StreamState,
    source: ImageSource,
    emitted: &mut Vec<UrpStreamEvent>,
) {
    let node_index = allocate_synthetic_node_index(state);
    emitted.push(UrpStreamEvent::NodeStart {
        node_index,
        header: NodeHeader::Image {
            metadata: Default::default(),

            id: None,
            role: OrdinaryRole::Assistant,
        },
        extra_body: HashMap::new(),
    });
    emitted.push(UrpStreamEvent::NodeDone {
        node_index,
        node: Node::Image {
            metadata: Default::default(),

            id: None,
            role: OrdinaryRole::Assistant,
            source,
            extra_body: HashMap::new(),
        },
        usage: None,
        extra_body: HashMap::new(),
    });
}

fn emit_node_segments(
    node_index: u32,
    state: &mut StreamState,
    node_state: &mut StreamTextNodeState,
    source: &str,
    segments: Vec<StreamSegment>,
    emitted: &mut Vec<UrpStreamEvent>,
    delta_extra_body: &HashMap<String, Value>,
) {
    let source_base = node_state.source_chars;
    node_state.source_chars += (source.chars().count()
        - node_state.buffered_tail.chars().count()) as u64;
    for segment in segments {
        match segment {
            StreamSegment::Text {
                content: text,
                source_start,
            } => {
                if text.is_empty() {
                    continue;
                }
                ensure_text_node_start(node_index, node_state, emitted);
                let span = kept_span(
                    source,
                    source_base,
                    source_start,
                    &text,
                    &node_state.cleaned_content,
                );
                node_state.spans.push(span);
                node_state.cleaned_content.push_str(&text);
                emitted.push(UrpStreamEvent::NodeDelta {
                    node_index,
                    delta: NodeDelta::Text {
                        logprobs: None,
                        signature: None,
                        citations: Vec::new(),
                        content: text,
                    },
                    usage: None,
                    extra_body: delta_extra_body.clone(),
                });
            }
            StreamSegment::Image(source) => emit_synthetic_image_node(state, source, emitted),
        }
    }
}

/// AMIO-13: moves citations whose range was parsed into cleaned-text coordinates.
fn place_citations(
    node_index: u32,
    node_state: &mut StreamTextNodeState,
    terminal: bool,
    emitted: &mut Vec<UrpStreamEvent>,
) {
    for mut citation in std::mem::take(&mut node_state.pending_citations) {
        let ready = terminal
            || (node_state.start_emitted
                && citation
                    .answer_range
                    .as_ref()
                    .is_none_or(|range| range.start < node_state.source_chars));
        if !ready {
            node_state.pending_citations.push(citation);
            continue;
        }
        // No text remains to cite.
        if !node_state.start_emitted || rebase_citation(&mut citation, &node_state.spans).is_none()
        {
            continue;
        }
        node_state.citations.push(citation.clone());
        emitted.push(UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Text {
                logprobs: None,
                signature: None,
                citations: vec![citation],
                content: String::new(),
            },
            usage: None,
            extra_body: HashMap::new(),
        });
    }
}

fn attach_usage_to_last_node_event(
    events: &mut [UrpStreamEvent],
    usage: Option<crate::urp::Usage>,
) {
    let Some(usage) = usage else {
        return;
    };
    let Some(last) = events.last_mut() else {
        return;
    };
    match last {
        UrpStreamEvent::NodeDelta { usage: slot, .. }
        | UrpStreamEvent::NodeDone { usage: slot, .. }
        | UrpStreamEvent::ResponseDone { usage: slot, .. } => *slot = Some(usage),
        _ => {}
    }
}

fn parse_markdown_image_source(url: &str) -> Option<ImageSource> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        if !meta.ends_with(";base64") {
            return None;
        }
        let media_type = meta.trim_end_matches(";base64");
        if !media_type.starts_with("image/") || data.is_empty() {
            return None;
        }
        return Some(ImageSource::Base64 {
            media_type: media_type.to_string(),
            data: data.to_string(),
        });
    }
    Some(ImageSource::Url {
        url: url.to_string(),
        detail: None,
    })
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ImageMarkdownToOutputTransform),
});

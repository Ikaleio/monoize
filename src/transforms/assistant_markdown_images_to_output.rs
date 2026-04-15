use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData, response_output_items_mut,
};
use crate::urp::{
    ImageSource, Item, ItemHeader, Node, NodeDelta, NodeHeader, Part, PartDelta, PartHeader,
    Role, UrpStreamEvent,
};
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

pub struct AssistantMarkdownImagesToOutputTransform;

struct StreamItemState {
    role: Role,
    extra_body: HashMap<String, Value>,
    start_seen_or_emitted: bool,
}

struct ActiveTextPart {
    part_index: u32,
    content: String,
}

struct StreamTextPartState {
    item_index: u32,
    source_part_index: u32,
    source_part_used: bool,
    part_extra_body: HashMap<String, Value>,
    buffered_tail: String,
    active_text_part: Option<ActiveTextPart>,
    saw_delta: bool,
}

struct StreamState {
    replacement: Option<Vec<UrpStreamEvent>>,
    item_states: HashMap<u32, StreamItemState>,
    text_parts: HashMap<u32, StreamTextPartState>,
    pending_synthetic_item_done: Vec<u32>,
    next_synthetic_item_index: u32,
    next_synthetic_part_index: u32,
    node_text_parts: HashMap<u32, StreamTextPartState>,
    pending_synthetic_node_done: Vec<u32>,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            replacement: None,
            item_states: HashMap::new(),
            text_parts: HashMap::new(),
            pending_synthetic_item_done: Vec::new(),
            next_synthetic_item_index: u32::MAX,
            next_synthetic_part_index: u32::MAX,
            node_text_parts: HashMap::new(),
            pending_synthetic_node_done: Vec::new(),
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
impl Transform for AssistantMarkdownImagesToOutputTransform {
    fn type_id(&self) -> &'static str {
        "assistant_markdown_images_to_output"
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
                let mut items = response_output_items_mut(resp);
                for item in items.iter_mut() {
                    rewrite_assistant_markdown_images(item);
                }
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

fn rewrite_assistant_markdown_images(item: &mut Item) {
    let Item::Message { role, parts, .. } = item else {
        return;
    };
    if *role != Role::Assistant {
        return;
    }
    let mut next_parts = Vec::with_capacity(parts.len());
    for part in parts.iter() {
        match part {
            Part::Text {
                content,
                extra_body,
            } => {
                let (cleaned, images) = extract_markdown_images_from_text(content);
                if !cleaned.is_empty() {
                    next_parts.push(Part::Text {
                        content: cleaned,
                        extra_body: extra_body.clone(),
                    });
                }
                next_parts.extend(images);
            }
            other => next_parts.push(other.clone()),
        }
    }
    *parts = next_parts;
}

fn rewrite_assistant_markdown_images_nodes(nodes: &mut Vec<Node>) {
    let mut rewritten = Vec::with_capacity(nodes.len());
    for node in nodes.drain(..) {
        match node {
            Node::Text {
                id,
                role: crate::urp::OrdinaryRole::Assistant,
                content,
                phase,
                extra_body,
            } => {
                let (cleaned, images) = extract_markdown_images_from_text(&content);
                if !cleaned.is_empty() {
                    rewritten.push(Node::Text {
                        id,
                        role: crate::urp::OrdinaryRole::Assistant,
                        content: cleaned,
                        phase,
                        extra_body,
                    });
                }
                for image in images {
                    let Part::Image { source, extra_body } = image else {
                        continue;
                    };
                    rewritten.push(Node::Image {
                        id: None,
                        role: crate::urp::OrdinaryRole::Assistant,
                        source,
                        extra_body,
                    });
                }
            }
            other => rewritten.push(other),
        }
    }
    *nodes = rewritten;
}

fn extract_markdown_images_from_text(content: &str) -> (String, Vec<Part>) {
    let (segments, tail) = split_stream_segments(content, true);
    debug_assert!(tail.is_empty());
    let mut images = Vec::new();
    let mut cleaned = String::new();
    for segment in segments {
        match segment {
            StreamSegment::Text(text) => cleaned.push_str(&text),
            StreamSegment::Image(source) => images.push(Part::Image {
                source,
                extra_body: HashMap::new(),
            }),
        }
    }
    (cleaned, images)
}

#[derive(Debug)]
enum StreamSegment {
    Text(String),
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
    let mut segments = Vec::new();

    while i < bytes.len() {
        if bytes[i] == b'!' && bytes.get(i + 1) == Some(&b'[') {
            match parse_markdown_candidate(content, i) {
                CandidateParse::Valid { end, source } => {
                    if !text.is_empty() {
                        segments.push(StreamSegment::Text(std::mem::take(&mut text)));
                    }
                    segments.push(StreamSegment::Image(source));
                    i = end;
                    continue;
                }
                CandidateParse::Invalid { safe_end } => {
                    text.push_str(&content[i..safe_end]);
                    i = safe_end;
                    continue;
                }
                CandidateParse::Incomplete => break,
            }
        }

        let ch = content[i..].chars().next().expect("stream parser char");
        text.push(ch);
        i += ch.len_utf8();
    }

    let mut tail = if i < bytes.len() {
        content[i..].to_string()
    } else {
        String::new()
    };
    if terminal && !tail.is_empty() {
        text.push_str(&tail);
        tail.clear();
    }
    if !text.is_empty() {
        segments.push(StreamSegment::Text(text));
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
        }
            | UrpStreamEvent::NodeDone {
                node: Node::NextDownstreamEnvelopeExtra { .. },
                ..
            }
    ) {
        state.replacement = Some(vec![event.clone()]);
        return;
    }
    if apply_node_stream(event, state) {
        return;
    }
    match event {
        UrpStreamEvent::ItemStart {
            item_index,
            header: ItemHeader::Message { role, .. },
            extra_body,
        } if *role == Role::Assistant => {
            state.item_states.insert(
                *item_index,
                StreamItemState {
                    role: *role,
                    extra_body: extra_body.clone(),
                    start_seen_or_emitted: true,
                },
            );
        }
        UrpStreamEvent::PartStart {
            part_index,
            item_index,
            header: PartHeader::Text,
            extra_body,
        } => {
            ensure_item_state(state, *item_index, HashMap::new(), true);
            state.text_parts.insert(
                *part_index,
                StreamTextPartState {
                    item_index: *item_index,
                    source_part_index: *part_index,
                    source_part_used: false,
                    part_extra_body: extra_body.clone(),
                    buffered_tail: String::new(),
                    active_text_part: None,
                    saw_delta: false,
                },
            );
            state.replacement = Some(Vec::new());
        }
        UrpStreamEvent::Delta {
            part_index,
            delta: PartDelta::Text { content },
            usage,
            extra_body,
        } => {
            let mut part_state =
                state
                    .text_parts
                    .remove(part_index)
                    .unwrap_or_else(|| StreamTextPartState {
                        item_index: allocate_synthetic_item_for_delta(state, extra_body.clone()),
                        source_part_index: *part_index,
                        source_part_used: false,
                        part_extra_body: extra_body.clone(),
                        buffered_tail: String::new(),
                        active_text_part: None,
                        saw_delta: false,
                    });
            part_state.saw_delta = true;
            let combined = format!("{}{}", part_state.buffered_tail, content);
            let (segments, tail) = split_stream_segments(&combined, false);
            part_state.buffered_tail = tail;
            let mut emitted = Vec::new();
            emit_segments(state, &mut part_state, segments, &mut emitted, extra_body);
            attach_usage_to_last_event(&mut emitted, usage.clone());
            state.text_parts.insert(*part_index, part_state);
            state.replacement = Some(emitted);
        }
        UrpStreamEvent::PartDone {
            part_index,
            part: Part::Text { content, .. },
            usage,
            ..
        } => {
            let Some(mut part_state) = state.text_parts.remove(part_index) else {
                return;
            };
            if !part_state.saw_delta {
                part_state.buffered_tail.push_str(content);
            }
            let (segments, tail) = split_stream_segments(&part_state.buffered_tail, true);
            part_state.buffered_tail = tail;
            let mut emitted = Vec::new();
            emit_segments(
                state,
                &mut part_state,
                segments,
                &mut emitted,
                &HashMap::new(),
            );
            close_active_text_part(&mut part_state, &mut emitted);
            attach_usage_to_last_event(&mut emitted, usage.clone());
            state.replacement = Some(emitted);
        }
        UrpStreamEvent::ItemDone {
            item_index,
            item,
            usage,
            extra_body,
        } => {
            let mut emitted = flush_open_parts_for_item(state, *item_index);
            rewrite_assistant_markdown_images(item);
            emitted.push(UrpStreamEvent::ItemDone {
                item_index: *item_index,
                item: item.clone(),
                usage: usage.clone(),
                extra_body: extra_body.clone(),
            });
            state.item_states.remove(item_index);
            state.replacement = Some(emitted);
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            rewrite_assistant_markdown_images_nodes(output);
            let mut emitted = flush_all_open_parts(state);
            for item_index in state.pending_synthetic_item_done.drain(..) {
                state.item_states.remove(&item_index);
            }
            for item_index in state.pending_synthetic_node_done.drain(..) {
                state.item_states.remove(&item_index);
            }
            emitted.push(event.clone());
            state.replacement = Some(emitted);
        }
        _ => {}
    }
}

fn apply_node_stream(event: &mut UrpStreamEvent, state: &mut StreamState) -> bool {
    match event {
        UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::Text { role, .. },
            extra_body,
        } if *role == crate::urp::OrdinaryRole::Assistant => {
            let item_index = allocate_synthetic_item_for_node(state, extra_body.clone());
            state.node_text_parts.insert(
                *node_index,
                StreamTextPartState {
                    item_index,
                    source_part_index: *node_index,
                    source_part_used: false,
                    part_extra_body: extra_body.clone(),
                    buffered_tail: String::new(),
                    active_text_part: None,
                    saw_delta: false,
                },
            );
            state.replacement = Some(Vec::new());
            true
        }
        UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Text { content },
            usage,
            extra_body,
        } => {
            let Some(mut part_state) = state.node_text_parts.remove(node_index) else {
                return false;
            };
            part_state.saw_delta = true;
            let combined = format!("{}{}", part_state.buffered_tail, content);
            let (segments, tail) = split_stream_segments(&combined, false);
            part_state.buffered_tail = tail;
            let mut emitted = Vec::new();
            emit_segments(state, &mut part_state, segments, &mut emitted, extra_body);
            attach_usage_to_last_event(&mut emitted, usage.clone());
            state.node_text_parts.insert(*node_index, part_state);
            state.replacement = Some(emitted);
            true
        }
        UrpStreamEvent::NodeDone {
            node_index,
            node: Node::Text { content, .. },
            usage,
            ..
        } => {
            let Some(mut part_state) = state.node_text_parts.remove(node_index) else {
                return false;
            };
            if !part_state.saw_delta {
                part_state.buffered_tail.push_str(content);
            }
            let (segments, tail) = split_stream_segments(&part_state.buffered_tail, true);
            part_state.buffered_tail = tail;
            let mut emitted = Vec::new();
            emit_segments(
                state,
                &mut part_state,
                segments,
                &mut emitted,
                &HashMap::new(),
            );
            close_active_text_part(&mut part_state, &mut emitted);
            attach_usage_to_last_event(&mut emitted, usage.clone());
            emitted.push(event.clone());
            state.replacement = Some(emitted);
            true
        }
        _ => false,
    }
}

fn allocate_synthetic_item_for_node(
    state: &mut StreamState,
    extra_body: HashMap<String, Value>,
) -> u32 {
    let item_index = allocate_synthetic_item_for_delta(state, extra_body);
    state.pending_synthetic_node_done.push(item_index);
    item_index
}

fn ensure_item_state(
    state: &mut StreamState,
    item_index: u32,
    extra_body: HashMap<String, Value>,
    start_seen_or_emitted: bool,
) {
    state
        .item_states
        .entry(item_index)
        .or_insert_with(|| StreamItemState {
            role: Role::Assistant,
            extra_body,
            start_seen_or_emitted,
        });
}

fn allocate_synthetic_item_for_delta(
    state: &mut StreamState,
    extra_body: HashMap<String, Value>,
) -> u32 {
    let item_index = state.next_synthetic_item_index;
    state.next_synthetic_item_index = state.next_synthetic_item_index.saturating_sub(1);
    state.item_states.insert(
        item_index,
        StreamItemState {
            role: Role::Assistant,
            extra_body,
            start_seen_or_emitted: false,
        },
    );
    state.pending_synthetic_item_done.push(item_index);
    item_index
}

fn allocate_synthetic_part_index(state: &mut StreamState) -> u32 {
    let part_index = state.next_synthetic_part_index;
    state.next_synthetic_part_index = state.next_synthetic_part_index.saturating_sub(1);
    part_index
}

fn ensure_item_start_event(
    state: &mut StreamState,
    item_index: u32,
    emitted: &mut Vec<UrpStreamEvent>,
) {
    let Some(item_state) = state.item_states.get_mut(&item_index) else {
        return;
    };
    if item_state.start_seen_or_emitted {
        return;
    }
    emitted.push(UrpStreamEvent::ItemStart {
        item_index,
        header: ItemHeader::Message {
            id: Some(crate::urp::synthetic_message_id()),
            role: item_state.role,
        },
        extra_body: item_state.extra_body.clone(),
    });
    item_state.start_seen_or_emitted = true;
}

fn ensure_active_text_part(
    state: &mut StreamState,
    part_state: &mut StreamTextPartState,
    emitted: &mut Vec<UrpStreamEvent>,
) {
    if part_state.active_text_part.is_some() {
        return;
    }
    ensure_item_start_event(state, part_state.item_index, emitted);
    let part_index = if !part_state.source_part_used {
        part_state.source_part_used = true;
        part_state.source_part_index
    } else {
        allocate_synthetic_part_index(state)
    };
    emitted.push(UrpStreamEvent::PartStart {
        part_index,
        item_index: part_state.item_index,
        header: PartHeader::Text,
        extra_body: part_state.part_extra_body.clone(),
    });
    part_state.active_text_part = Some(ActiveTextPart {
        part_index,
        content: String::new(),
    });
}

fn emit_segments(
    state: &mut StreamState,
    part_state: &mut StreamTextPartState,
    segments: Vec<StreamSegment>,
    emitted: &mut Vec<UrpStreamEvent>,
    delta_extra_body: &HashMap<String, Value>,
) {
    for segment in segments {
        match segment {
            StreamSegment::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                ensure_active_text_part(state, part_state, emitted);
                if let Some(active) = part_state.active_text_part.as_mut() {
                    active.content.push_str(&text);
                    emitted.push(UrpStreamEvent::Delta {
                        part_index: active.part_index,
                        delta: PartDelta::Text { content: text },
                        usage: None,
                        extra_body: delta_extra_body.clone(),
                    });
                }
            }
            StreamSegment::Image(source) => {
                close_active_text_part(part_state, emitted);
                ensure_item_start_event(state, part_state.item_index, emitted);
                let part_index = allocate_synthetic_part_index(state);
                emitted.push(UrpStreamEvent::PartStart {
                    part_index,
                    item_index: part_state.item_index,
                    header: PartHeader::Image {
                        extra_body: HashMap::new(),
                    },
                    extra_body: HashMap::new(),
                });
                emitted.push(UrpStreamEvent::PartDone {
                    part_index,
                    part: Part::Image {
                        source,
                        extra_body: HashMap::new(),
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                });
            }
        }
    }
}

fn close_active_text_part(part_state: &mut StreamTextPartState, emitted: &mut Vec<UrpStreamEvent>) {
    let Some(active) = part_state.active_text_part.take() else {
        return;
    };
    emitted.push(UrpStreamEvent::PartDone {
        part_index: active.part_index,
        part: Part::Text {
            content: active.content,
            extra_body: part_state.part_extra_body.clone(),
        },
        usage: None,
        extra_body: HashMap::new(),
    });
}

fn flush_open_parts_for_item(state: &mut StreamState, item_index: u32) -> Vec<UrpStreamEvent> {
    let matching_part_indices = state
        .text_parts
        .iter()
        .filter_map(|(part_index, part_state)| {
            (part_state.item_index == item_index).then_some(*part_index)
        })
        .collect::<Vec<_>>();
    let mut emitted = Vec::new();
    for part_index in matching_part_indices {
        if let Some(mut part_state) = state.text_parts.remove(&part_index) {
            let (segments, tail) = split_stream_segments(&part_state.buffered_tail, true);
            part_state.buffered_tail = tail;
            emit_segments(
                state,
                &mut part_state,
                segments,
                &mut emitted,
                &HashMap::new(),
            );
            close_active_text_part(&mut part_state, &mut emitted);
        }
    }
    emitted
}

fn flush_all_open_parts(state: &mut StreamState) -> Vec<UrpStreamEvent> {
    let part_indices = state.text_parts.keys().copied().collect::<Vec<_>>();
    let mut emitted = Vec::new();
    for part_index in part_indices {
        if let Some(mut part_state) = state.text_parts.remove(&part_index) {
            let (segments, tail) = split_stream_segments(&part_state.buffered_tail, true);
            part_state.buffered_tail = tail;
            emit_segments(
                state,
                &mut part_state,
                segments,
                &mut emitted,
                &HashMap::new(),
            );
            close_active_text_part(&mut part_state, &mut emitted);
        }
    }
    emitted
}

fn attach_usage_to_last_event(events: &mut [UrpStreamEvent], usage: Option<crate::urp::Usage>) {
    let Some(usage) = usage else {
        return;
    };
    let Some(last) = events.last_mut() else {
        return;
    };
    match last {
        UrpStreamEvent::Delta { usage: slot, .. }
        | UrpStreamEvent::PartDone { usage: slot, .. }
        | UrpStreamEvent::ItemDone { usage: slot, .. }
        | UrpStreamEvent::NodeDelta { usage: slot, .. }
        | UrpStreamEvent::NodeDone { usage: slot, .. }
        | UrpStreamEvent::ResponseDone { usage: slot, .. } => *slot = Some(usage),
        UrpStreamEvent::ResponseStart { .. }
        | UrpStreamEvent::ItemStart { .. }
        | UrpStreamEvent::PartStart { .. }
        | UrpStreamEvent::NodeStart { .. }
        | UrpStreamEvent::Error { .. } => {}
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
    factory: || Box::new(AssistantMarkdownImagesToOutputTransform),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_url_and_base64_markdown_images() {
        let (cleaned, images) = extract_markdown_images_from_text(
            "hello ![a](https://example.com/a.png) world ![b](data:image/png;base64,QUJD)",
        );
        assert_eq!(cleaned, "hello  world ");
        assert_eq!(images.len(), 2);
        match &images[0] {
            Part::Image {
                source: ImageSource::Url { url, .. },
                ..
            } => assert_eq!(url, "https://example.com/a.png"),
            _ => panic!("expected url image"),
        }
        match &images[1] {
            Part::Image {
                source: ImageSource::Base64 { media_type, data },
                ..
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "QUJD");
            }
            _ => panic!("expected base64 image"),
        }
    }
}

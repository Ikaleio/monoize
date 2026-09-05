use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

const DEFAULT_TITLE: &str = "Thinking";
const DEFAULT_MAX_TITLE_CHARS: usize = 64;

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default = "default_title")]
    default_title: String,
    #[serde(default)]
    derive_title: bool,
    #[serde(default = "default_max_title_chars")]
    max_title_chars: usize,
}

fn default_title() -> String {
    DEFAULT_TITLE.to_string()
}

fn default_max_title_chars() -> usize {
    DEFAULT_MAX_TITLE_CHARS
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct NodeStreamState {
    summary_delta_seen: bool,
    heading_prefixed: bool,
}

#[derive(Default)]
struct StreamState {
    replacement: Option<Vec<UrpStreamEvent>>,
    nodes: HashMap<u32, NodeStreamState>,
}

impl TransformState for StreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn finalize_stream_event(&mut self, event: UrpStreamEvent) -> Vec<UrpStreamEvent> {
        self.replacement.take().unwrap_or_else(|| vec![event])
    }
}

pub struct ReasoningSummaryHeadingTransform;

#[async_trait]
impl Transform for ReasoningSummaryHeadingTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_summary_heading"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: summary heading"),
            ("zh", "推理：摘要标题"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Prefixes untitled reasoning summaries with a **title** heading and injects a catch-up summary delta when a reasoning node completes without a prior summary delta.",
            ),
            (
                "zh",
                "为无标题的推理摘要添加 **title** 前缀，并在推理节点结束且此前没有摘要增量时补发一条摘要增量。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[
            TransformScope::Provider,
            TransformScope::Global,
            TransformScope::ApiKey,
        ]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "default_title": {
                    "type": "string",
                    "default": DEFAULT_TITLE
                },
                "derive_title": {
                    "type": "boolean",
                    "default": false
                },
                "max_title_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "default": DEFAULT_MAX_TITLE_CHARS
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let mut cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        if cfg.max_title_chars == 0 {
            return Err(TransformError::InvalidConfig(
                "max_title_chars must be >= 1".to_string(),
            ));
        }
        cfg.default_title = sanitize_title(&cfg.default_title, cfg.max_title_chars);
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
                    apply_heading_to_reasoning_node(node, cfg, false);
                }
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<StreamState>() else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                apply_stream(event, cfg, stream_state);
            }
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn apply_stream(event: &mut UrpStreamEvent, cfg: &Config, state: &mut StreamState) {
    let mut injected_delta = None;
    match event {
        UrpStreamEvent::NodeDelta {
            node_index, delta, ..
        } => {
            apply_live_summary_delta(*node_index, delta, cfg, state);
        }
        UrpStreamEvent::NodeDone {
            node_index, node, ..
        } => {
            injected_delta = inject_nodedone_summary_delta(*node_index, node, cfg, state);
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                apply_heading_to_reasoning_node(node, cfg, false);
            }
        }
        _ => {}
    }
    if let Some(delta) = injected_delta {
        state.replacement = Some(vec![delta, event.clone()]);
    }
}

fn apply_live_summary_delta(
    node_index: u32,
    delta: &mut NodeDelta,
    cfg: &Config,
    state: &mut StreamState,
) {
    let NodeDelta::Reasoning { summary, .. } = delta else {
        return;
    };
    let Some(text) = summary.as_mut() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let node_state = state.nodes.entry(node_index).or_default();
    if node_state.summary_delta_seen {
        return;
    }
    node_state.summary_delta_seen = true;
    if !has_heading(text) {
        *text = prefix_heading(&cfg.default_title, text);
    }
    node_state.heading_prefixed = true;
}

fn inject_nodedone_summary_delta(
    node_index: u32,
    node: &mut Node,
    cfg: &Config,
    state: &mut StreamState,
) -> Option<UrpStreamEvent> {
    let Node::Reasoning {
        summary, source, ..
    } = node
    else {
        return None;
    };
    let node_state = state.nodes.entry(node_index).or_default();
    apply_heading_to_summary(summary, cfg, node_state.heading_prefixed);
    let text = summary.as_ref().filter(|text| !text.is_empty())?;
    if node_state.summary_delta_seen {
        return None;
    }
    // NodeDone is the item lifecycle the Responses encoder maps to output_item.done.
    // A catch-up summary delta must precede it when this node never streamed one.
    Some(UrpStreamEvent::NodeDelta {
        node_index,
        delta: NodeDelta::Reasoning {
            content: None,
            encrypted: None,
            summary: Some(text.clone()),
            source: source.clone(),
        },
        usage: None,
        extra_body: HashMap::new(),
    })
}

fn apply_heading_to_reasoning_node(node: &mut Node, cfg: &Config, keep_live_heading: bool) {
    let Node::Reasoning { summary, .. } = node else {
        return;
    };
    apply_heading_to_summary(summary, cfg, keep_live_heading);
}

fn apply_heading_to_summary(summary: &mut Option<String>, cfg: &Config, keep_live_heading: bool) {
    let Some(text) = summary.as_mut() else {
        return;
    };
    if text.is_empty() || has_heading(text) {
        return;
    }
    let title = if keep_live_heading || !cfg.derive_title {
        cfg.default_title.clone()
    } else {
        sanitize_title(&derive_title_candidate(text), cfg.max_title_chars)
    };
    *text = prefix_heading(&title, text);
}

// Codex split_reasoning_summary_parts treats a heading as **title** immediately
// followed by a newline. Mid-sentence **emphasis** therefore stays untitled.
fn has_heading(summary: &str) -> bool {
    let text = summary.trim();
    let Some(rest) = text.strip_prefix("**") else {
        return false;
    };
    let Some(close) = rest.find("**") else {
        return false;
    };
    if close == 0 {
        return false;
    }
    let after = &rest[close + 2..];
    after.starts_with('\n') || after.starts_with('\r')
}

fn prefix_heading(title: &str, body: &str) -> String {
    let mut out = String::with_capacity(title.len() + body.len() + 6);
    out.push_str("**");
    out.push_str(title);
    out.push_str("**\n\n");
    out.push_str(body);
    out
}

fn derive_title_candidate(body: &str) -> &str {
    let body = body.trim();
    let mut iter = body.char_indices();
    let Some(_) = iter.next() else {
        return "";
    };
    for (idx, ch) in iter {
        if matches!(ch, '.' | '!' | '?' | '\n' | '\r') {
            return &body[..idx];
        }
    }
    body
}

fn sanitize_title(raw: &str, max_title_chars: usize) -> String {
    let collapsed = raw.trim().replace(['\n', '\r'], " ").replace('*', "");
    let truncated = truncate_at_limit(&collapsed, max_title_chars);
    let trimmed = truncated.trim();
    if trimmed.is_empty() {
        DEFAULT_TITLE.to_string()
    } else {
        trimmed.to_string()
    }
}

fn truncate_at_limit(text: &str, max_title_chars: usize) -> String {
    if text.chars().count() <= max_title_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_title_chars).collect();
    match truncated.rfind(char::is_whitespace) {
        Some(idx) => truncated[..idx].to_string(),
        None => truncated,
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningSummaryHeadingTransform),
});

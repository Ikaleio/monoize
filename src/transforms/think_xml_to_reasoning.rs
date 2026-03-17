use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData, response_output_items_mut,
};
use async_trait::async_trait;
use crate::urp::{Item, Part, PartDelta, PartHeader, UrpStreamEvent};
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

#[derive(Default)]
struct StreamState {
    in_reasoning: HashMap<u32, bool>,
}

impl TransformState for StreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ThinkXmlToReasoningTransform;

#[async_trait]
impl Transform for ThinkXmlToReasoningTransform {
    fn type_id(&self) -> &'static str {
        "think_xml_to_reasoning"
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
                for message in response_output_items_mut(resp) {
                    if let Item::Message { parts, .. } = message {
                        let mut out = Vec::new();
                        for part in parts.iter() {
                            if let Part::Text { content, .. } = part {
                                let parsed = extract_text_and_reasoning(&content, &cfg.tag);
                                for piece in parsed {
                                    out.push(piece);
                                }
                            } else {
                                out.push(part.clone());
                            }
                        }
                        *parts = out;
                    }
                }
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

fn extract_text_and_reasoning(content: &str, tag: &str) -> Vec<Part> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut parts = Vec::new();
    let mut rest = content;

    loop {
        let Some(start) = rest.find(&open) else {
            if !rest.is_empty() {
                parts.push(Part::Text {
                    content: rest.to_string(),
                    extra_body: HashMap::new(),
                });
            }
            break;
        };
        let before = &rest[..start];
        if !before.is_empty() {
            parts.push(Part::Text {
                content: before.to_string(),
                extra_body: HashMap::new(),
            });
        }
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(&close) else {
            if !after_open.is_empty() {
                parts.push(Part::Reasoning {
                    content: Some(after_open.to_string()),
                    encrypted: None,
                    summary: None,
                    source: None,
                    extra_body: HashMap::new(),
                });
            }
            break;
        };
        let reasoning = &after_open[..end];
        if !reasoning.is_empty() {
            parts.push(Part::Reasoning {
                content: Some(reasoning.to_string()),
                encrypted: None,
                summary: None,
                source: None,
                extra_body: HashMap::new(),
            });
        }
        rest = &after_open[end + close.len()..];
    }
    parts
}

fn apply_stream(event: &mut UrpStreamEvent, state: &mut StreamState, tag: &str) {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    match event {
        UrpStreamEvent::PartStart {
            item_index,
            part_index,
            header,
            ..
        } => {
            if matches!(header, PartHeader::Text) {
                state
                    .in_reasoning
                    .insert((*item_index << 16) | *part_index, false);
            }
        }
        UrpStreamEvent::Delta {
            part_index,
            delta,
            ..
        } => {
            let key = *part_index;
            let Some(in_reasoning) = state.in_reasoning.get_mut(&key) else {
                return;
            };
            if let PartDelta::Text { content } = delta {
                if content.contains(&open) || *in_reasoning {
                    let mut s = content.clone();
                    if let Some(pos) = s.find(&open) {
                        s = s[(pos + open.len())..].to_string();
                        *in_reasoning = true;
                    }
                    if let Some(end) = s.find(&close) {
                        s = s[..end].to_string();
                        *in_reasoning = false;
                    }
                    *delta = PartDelta::Reasoning {
                        content: Some(s),
                        encrypted: None,
                        summary: None,
                        source: None,
                    };
                }
            }
        }
        UrpStreamEvent::PartDone {
            part_index,
            ..
        } => {
            state.in_reasoning.remove(part_index);
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ThinkXmlToReasoningTransform),
});

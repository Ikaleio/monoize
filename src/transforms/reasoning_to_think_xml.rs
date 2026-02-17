use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState,
    UrpData,
};
use crate::urp::{Part, PartDelta, PartHeader, UrpStreamEvent};
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

pub struct ReasoningToThinkXmlTransform;

impl Transform for ReasoningToThinkXmlTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_to_think_xml"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
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
        Box::new(NoState)
    }

    fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        match data {
            UrpData::Response(resp) => {
                let mut next_parts = Vec::with_capacity(resp.message.parts.len());
                for part in &resp.message.parts {
                    match part {
                        Part::Reasoning { content, .. } => {
                            next_parts.push(Part::Text {
                                content: format!("<{0}>{1}</{0}>", cfg.tag, content),
                                extra_body: HashMap::new(),
                            });
                        }
                        other => next_parts.push(other.clone()),
                    }
                }
                resp.message.parts = next_parts;
            }
            UrpData::Stream(event) => convert_stream_reasoning_to_xml(event, &cfg.tag),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn convert_stream_reasoning_to_xml(event: &mut UrpStreamEvent, tag: &str) {
    match event {
        UrpStreamEvent::PartStart { part, .. } => {
            if matches!(part, PartHeader::Reasoning) {
                *part = PartHeader::Text;
            }
        }
        UrpStreamEvent::Delta { delta, .. } => {
            if let PartDelta::Reasoning { content } = delta {
                *delta = PartDelta::Text {
                    content: format!("<{0}>{1}</{0}>", tag, content),
                };
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningToThinkXmlTransform),
});

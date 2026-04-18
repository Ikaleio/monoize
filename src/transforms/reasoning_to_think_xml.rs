use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
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

pub struct ReasoningToThinkXmlTransform;

#[async_trait]
impl Transform for ReasoningToThinkXmlTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_to_think_xml"
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
        Box::new(NoState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        match data {
            UrpData::Response(resp) => {
                for node in &mut resp.output {
                    if let Node::Reasoning {
                        content: Some(content),
                        ..
                    } = node
                    {
                        *node = Node::Text {
                            id: None,
                            role: OrdinaryRole::Assistant,
                            content: format!("<{0}>{1}</{0}>", cfg.tag, content),
                            phase: None,
                            extra_body: HashMap::new(),
                        };
                    }
                }
            }
            UrpData::Stream(event) => convert_stream_reasoning_to_xml(event, &cfg.tag),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn convert_stream_reasoning_to_xml(event: &mut UrpStreamEvent, tag: &str) {
    match event {
        UrpStreamEvent::NodeStart { header, .. } => {
            if let NodeHeader::Reasoning { id } = header {
                *header = NodeHeader::Text {
                    id: id.take(),
                    role: OrdinaryRole::Assistant,
                    phase: None,
                };
            }
        }
        UrpStreamEvent::NodeDelta { delta, .. } => {
            if let NodeDelta::Reasoning {
                content: Some(content),
                encrypted: None,
                summary: None,
                ..
            } = delta
            {
                *delta = NodeDelta::Text {
                    content: format!("<{tag}>{content}</{tag}>"),
                };
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningToThinkXmlTransform),
});

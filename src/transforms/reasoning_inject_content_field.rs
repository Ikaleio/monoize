use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct NoOpState;

impl TransformState for NoOpState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ReasoningInjectContentFieldTransform;

#[async_trait]
impl Transform for ReasoningInjectContentFieldTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_inject_content_field"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: inject reasoning_content field"),
            ("zh", "推理：注入 reasoning_content 字段"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Marks reasoning nodes and deltas so Chat Completions encoders emit OpenRouter/DeepSeek-compatible reasoning_content fields.",
            ),
            (
                "zh",
                "标记推理节点与增量，使 Chat Completions 编码器输出 OpenRouter/DeepSeek 兼容的 reasoning_content 字段。",
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
        Box::new(NoOpState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        match data {
            UrpData::Response(resp) => {
                for node in &mut resp.output {
                    mark_node(node);
                }
            }
            UrpData::Stream(event) => mark_stream(event),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn extract_reasoning_content(content: &Option<String>, summary: &Option<String>) -> Option<String> {
    if let Some(content) = content {
        if !content.is_empty() {
            return Some(content.clone());
        }
    }
    if let Some(sum) = summary {
        if !sum.is_empty() {
            return Some(sum.clone());
        }
    }
    None
}

fn mark_node(node: &mut Node) {
    let Node::Reasoning {
        content,
        encrypted,
        summary,
        extra_body,
        ..
    } = node
    else {
        return;
    };
    let _ = encrypted;
    if let Some(value) = extract_reasoning_content(content, summary) {
        extra_body.insert("inject_reasoning_content".to_string(), Value::String(value));
    }
}

fn mark_stream(event: &mut UrpStreamEvent) {
    match event {
        UrpStreamEvent::NodeDelta {
            delta:
                NodeDelta::Reasoning {
                    content,
                    encrypted,
                    summary,
                    ..
                },
            extra_body,
            ..
        } => {
            let _ = encrypted;
            if let Some(value) = extract_reasoning_content(content, summary) {
                extra_body.insert("inject_reasoning_content".to_string(), Value::String(value));
            }
        }
        UrpStreamEvent::NodeDone { node, .. } => {
            let Node::Reasoning {
                content,
                encrypted,
                summary,
                extra_body,
                ..
            } = node
            else {
                return;
            };
            let _ = encrypted;
            if let Some(value) = extract_reasoning_content(content, summary) {
                extra_body.insert("inject_reasoning_content".to_string(), Value::String(value));
            }
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                mark_node(node);
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningInjectContentFieldTransform),
});

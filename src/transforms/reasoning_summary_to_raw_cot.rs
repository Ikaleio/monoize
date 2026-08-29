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

pub struct ReasoningSummaryToRawCotTransform;

#[async_trait]
impl Transform for ReasoningSummaryToRawCotTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_summary_to_raw_cot"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: summary to raw CoT"),
            ("zh", "推理：摘要转原始思维链"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Marks reasoning summaries for OpenWebUI-compatible raw chain-of-thought emission by downstream Chat encoders.",
            ),
            (
                "zh",
                "标记推理摘要，使下游 Chat 编码器以 OpenWebUI 兼容的原始思维链字段输出。",
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

fn mark_node(node: &mut Node) {
    let Node::Reasoning {
        summary,
        extra_body,
        ..
    } = node
    else {
        return;
    };
    if summary
        .as_deref()
        .is_some_and(|summary| !summary.is_empty())
    {
        extra_body.insert("openwebui_reasoning_content".to_string(), Value::Bool(true));
    }
}

fn mark_stream(event: &mut UrpStreamEvent) {
    match event {
        UrpStreamEvent::NodeDelta {
            delta, extra_body, ..
        } => {
            if let NodeDelta::Reasoning { summary, .. } = delta
                && summary
                    .as_deref()
                    .is_some_and(|summary| !summary.is_empty())
            {
                extra_body.insert("openwebui_reasoning_content".to_string(), Value::Bool(true));
            }
        }
        UrpStreamEvent::NodeDone { node, .. } => mark_node(node),
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                mark_node(node);
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningSummaryToRawCotTransform),
});

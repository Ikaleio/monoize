use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashSet;

const SUMMARY_FROM_PLAINTEXT_REASONING_KEY: &str = "_monoize_summary_from_plaintext_reasoning";

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct StreamState {
    encrypted_nodes: HashSet<u32>,
}

impl TransformState for StreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ReasoningContentToSummaryTransform;

#[async_trait]
impl Transform for ReasoningContentToSummaryTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_content_to_summary"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: content to summary"),
            ("zh", "推理：正文转摘要"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Moves plaintext reasoning content into the reasoning summary field on responses and streams.",
            ),
            ("zh", "在响应与流中将明文推理正文移动到推理摘要字段。"),
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
                for node in &mut resp.output {
                    rewrite_reasoning_node(node);
                }
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<StreamState>() else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                rewrite_stream_reasoning(event, stream_state);
            }
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn rewrite_stream_reasoning(event: &mut UrpStreamEvent, state: &mut StreamState) {
    match event {
        UrpStreamEvent::NodeDelta {
            node_index,
            delta,
            extra_body,
            ..
        } => {
            let NodeDelta::Reasoning {
                content,
                encrypted,
                summary,
                ..
            } = delta
            else {
                return;
            };
            if encrypted.is_some() {
                state.encrypted_nodes.insert(*node_index);
            }
            if let Some(text) = content.take().filter(|text| !text.is_empty()) {
                *summary = Some(text);
                extra_body.insert(
                    SUMMARY_FROM_PLAINTEXT_REASONING_KEY.to_string(),
                    Value::Bool(true),
                );
            }
        }
        UrpStreamEvent::NodeDone {
            node_index, node, ..
        } => {
            if let Node::Reasoning { encrypted, .. } = node {
                if encrypted.is_some() {
                    state.encrypted_nodes.insert(*node_index);
                }
            }
            rewrite_reasoning_node(node);
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                rewrite_reasoning_node(node);
            }
        }
        _ => {}
    }
}

fn rewrite_reasoning_node(node: &mut Node) {
    let Node::Reasoning {
        content, summary, ..
    } = node
    else {
        return;
    };
    let Some(text) = content.take() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    *summary = Some(text);
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningContentToSummaryTransform),
});

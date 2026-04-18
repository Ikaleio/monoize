use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData, strip_reasoning_nodes,
};
use crate::urp::{Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct StripReasoningTransform;

#[derive(Default)]
struct StripState {
    stripped_indices: HashSet<u32>,
}

impl TransformState for StripState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[async_trait]
impl Transform for StripReasoningTransform {
    fn type_id(&self) -> &'static str {
        "strip_reasoning"
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
        Box::new(StripState::default())
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
                resp.output = strip_reasoning_nodes(&resp.output);
            }
            UrpData::Stream(event) => strip_stream_reasoning(event, state),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn strip_stream_reasoning(event: &mut UrpStreamEvent, state: &mut dyn TransformState) {
    let Some(strip_state) = state.as_any_mut().downcast_mut::<StripState>() else {
        return;
    };
    match event {
        UrpStreamEvent::NodeStart {
            node_index, header, ..
        } => {
            if let NodeHeader::Reasoning { id } = header {
                strip_state.stripped_indices.insert(*node_index);
                *header = NodeHeader::Text {
                    id: id.take(),
                    role: OrdinaryRole::Assistant,
                    phase: None,
                };
            }
        }
        UrpStreamEvent::NodeDelta {
            node_index, delta, ..
        } => {
            if strip_state.stripped_indices.contains(node_index)
                && matches!(delta, NodeDelta::Reasoning { .. })
            {
                *delta = NodeDelta::Text {
                    content: String::new(),
                };
            }
        }
        UrpStreamEvent::NodeDone {
            node_index, node, ..
        } => {
            if strip_state.stripped_indices.remove(node_index) {
                let (id, extra_body) = match node {
                    Node::Reasoning { id, extra_body, .. } => (id.take(), std::mem::take(extra_body)),
                    _ => (None, Default::default()),
                };
                *node = Node::Text {
                    id,
                    role: OrdinaryRole::Assistant,
                    content: String::new(),
                    phase: None,
                    extra_body,
                };
            }
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            *output = strip_reasoning_nodes(output);
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StripReasoningTransform),
});

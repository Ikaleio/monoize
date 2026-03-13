use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData, response_output_messages_mut, strip_reasoning_parts,
};
use async_trait::async_trait;
use crate::urp::{Part, PartDelta, PartHeader, UrpStreamEvent};
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
                for message in response_output_messages_mut(resp) {
                    message.parts = strip_reasoning_parts(&message.parts);
                }
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
        UrpStreamEvent::PartStart {
            message_index,
            part_index,
            header,
            ..
        } => {
            if matches!(header, PartHeader::Reasoning) {
                strip_state
                    .stripped_indices
                    .insert((*message_index << 16) | *part_index);
                *header = PartHeader::Text;
            }
        }
        UrpStreamEvent::Delta {
            part_index,
            delta,
            ..
        } => {
            let key = *part_index;
            if strip_state.stripped_indices.contains(&key)
                && matches!(delta, PartDelta::Reasoning { .. })
            {
                *delta = PartDelta::Text {
                    content: String::new(),
                };
            }
        }
        UrpStreamEvent::PartDone {
            part_index,
            part,
            ..
        } => {
            let key = *part_index;
            if strip_state.stripped_indices.contains(&key) {
                *part = Part::Text {
                    content: String::new(),
                    extra_body: Default::default(),
                };
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StripReasoningTransform),
});

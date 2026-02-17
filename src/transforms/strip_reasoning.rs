use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState, UrpData,
    strip_reasoning_parts,
};
use crate::urp::{PartDelta, PartHeader, UrpStreamEvent};
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

impl Transform for StripReasoningTransform {
    fn type_id(&self) -> &'static str {
        "strip_reasoning"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
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

    fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _config: &dyn TransformConfig,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        match data {
            UrpData::Response(resp) => {
                resp.message.parts = strip_reasoning_parts(&resp.message.parts);
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
            part_index, part, ..
        } => {
            if matches!(part, PartHeader::Reasoning | PartHeader::ReasoningEncrypted) {
                strip_state.stripped_indices.insert(*part_index);
                *part = PartHeader::Text;
            }
        }
        UrpStreamEvent::Delta {
            part_index, delta, ..
        } => {
            if strip_state.stripped_indices.contains(part_index) {
                match delta {
                    PartDelta::Reasoning { .. } | PartDelta::ReasoningEncrypted { .. } => {
                        *delta = PartDelta::Text {
                            content: String::new(),
                        };
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StripReasoningTransform),
});

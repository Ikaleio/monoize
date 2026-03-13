use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState,
    UrpData,
};
use async_trait::async_trait;
use crate::urp::Role;
use serde::Deserialize;
use serde_json::{json, Value};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct AutoCacheSystemTransform;

/// If the system prompt has no cache_control on any of its parts,
/// add cache_control: {type: "ephemeral"} to its last part.
/// Respects the max-4 cache breakpoint limit.
#[async_trait]
impl Transform for AutoCacheSystemTransform {
    fn type_id(&self) -> &'static str {
        "auto_cache_system"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
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
        Box::new(NoState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        if count_cache_breakpoints(req) >= 4 {
            return Ok(());
        }

        // Find the last system message
        let system_idx = req
            .inputs
            .iter()
            .rposition(|m| m.role == Role::System || m.role == Role::Developer);
        let Some(idx) = system_idx else {
            return Ok(());
        };

        // Check if any part of the system message already has cache_control
        let already_has_cache = req.inputs[idx]
            .parts
            .iter()
            .any(|p| part_extra_body(p).is_some_and(|eb| eb.contains_key("cache_control")));
        if already_has_cache {
            return Ok(());
        }

        // Add cache_control to the last part of the system message
        if let Some(last_part) = req.inputs[idx].parts.last_mut() {
            if let Some(eb) = part_extra_body_mut(last_part) {
                eb.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
            }
        }

        Ok(())
    }
}

fn count_cache_breakpoints(req: &crate::urp::UrpRequest) -> usize {
    req.inputs
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter(|p| part_extra_body(p).is_some_and(|eb| eb.contains_key("cache_control")))
        .count()
}

fn part_extra_body(part: &crate::urp::Part) -> Option<&std::collections::HashMap<String, Value>> {
    match part {
        crate::urp::Part::Text { extra_body, .. }
        | crate::urp::Part::Image { extra_body, .. }
        | crate::urp::Part::Audio { extra_body, .. }
        | crate::urp::Part::File { extra_body, .. }
        | crate::urp::Part::Reasoning { extra_body, .. }
        | crate::urp::Part::ToolCall { extra_body, .. }
        | crate::urp::Part::ToolResult { extra_body, .. }
        | crate::urp::Part::ProviderItem { extra_body, .. }
        | crate::urp::Part::Refusal { extra_body, .. } => Some(extra_body),
    }
}

fn part_extra_body_mut(
    part: &mut crate::urp::Part,
) -> Option<&mut std::collections::HashMap<String, Value>> {
    match part {
        crate::urp::Part::Text { extra_body, .. }
        | crate::urp::Part::Image { extra_body, .. }
        | crate::urp::Part::Audio { extra_body, .. }
        | crate::urp::Part::File { extra_body, .. }
        | crate::urp::Part::Reasoning { extra_body, .. }
        | crate::urp::Part::ToolCall { extra_body, .. }
        | crate::urp::Part::ToolResult { extra_body, .. }
        | crate::urp::Part::ProviderItem { extra_body, .. }
        | crate::urp::Part::Refusal { extra_body, .. } => Some(extra_body),
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(AutoCacheSystemTransform),
});

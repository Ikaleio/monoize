use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState,
    UrpData,
};
use async_trait::async_trait;
use crate::urp::{Part, Role};
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

pub struct AutoCacheToolUseTransform;

/// When the user returns tool results, find the last User message before
/// the Assistant's ToolCall and add cache_control to its last part.
/// This makes long tool-call chains benefit from caching.
/// Respects the max-4 cache breakpoint limit.
#[async_trait]
impl Transform for AutoCacheToolUseTransform {
    fn type_id(&self) -> &'static str {
        "auto_cache_tool_use"
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

        // Check if the last message is a Tool result
        let last_msg = req.inputs.last();
        let is_tool_result = last_msg.is_some_and(|m| {
            m.role == Role::Tool || m.parts.iter().any(|p| matches!(p, Part::ToolResult { .. }))
        });
        if !is_tool_result {
            return Ok(());
        }

        if count_cache_breakpoints(req) >= 4 {
            return Ok(());
        }

        // Walk backwards to find: Tool(result) <- Assistant(ToolCall) <- User(the target)
        // Find the last Assistant message with a ToolCall before the trailing tool results
        let mut assistant_tool_call_idx: Option<usize> = None;
        for (i, msg) in req.inputs.iter().enumerate().rev() {
            if msg.role == Role::Tool
                || msg
                    .parts
                    .iter()
                    .any(|p| matches!(p, Part::ToolResult { .. }))
            {
                continue;
            }
            if msg.role == Role::Assistant
                && msg.parts.iter().any(|p| matches!(p, Part::ToolCall { .. }))
            {
                assistant_tool_call_idx = Some(i);
                break;
            }
            // If we hit anything else (User, System), stop searching
            break;
        }

        let Some(assistant_idx) = assistant_tool_call_idx else {
            return Ok(());
        };

        // Find the last User message before the assistant's tool call
        let mut target_user_idx: Option<usize> = None;
        for i in (0..assistant_idx).rev() {
            if req.inputs[i].role == Role::User {
                target_user_idx = Some(i);
                break;
            }
        }

        let Some(user_idx) = target_user_idx else {
            return Ok(());
        };

        // Check if that User message already has cache_control
        let already_has_cache = req.inputs[user_idx]
            .parts
            .iter()
            .any(|p| part_extra_body(p).is_some_and(|eb| eb.contains_key("cache_control")));
        if already_has_cache {
            return Ok(());
        }

        // Add cache_control to the last part of the target User message
        if let Some(last_part) = req.inputs[user_idx].parts.last_mut() {
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
    factory: || Box::new(AutoCacheToolUseTransform),
});

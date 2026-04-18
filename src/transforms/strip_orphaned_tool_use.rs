use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::Node;
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

pub struct StripOrphanedToolUseTransform;

/// Anthropic requires every `tool_use` block to have a corresponding
/// `tool_result` immediately after. When conversations are truncated
/// or the last assistant turn contains tool calls without follow-up
/// results, the API rejects with 400. This transform collects all
/// `tool_result` call_ids in the conversation, then removes any
/// `Part::ToolCall` whose call_id has no matching result.
/// If removing all ToolCall parts from an assistant message leaves it
/// empty, the entire message is dropped.
#[async_trait]
impl Transform for StripOrphanedToolUseTransform {
    fn type_id(&self) -> &'static str {
        "strip_orphaned_tool_use"
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
        if let UrpData::Request(req) = data {
            let result_ids: HashSet<String> = req
                .input
                .iter()
                .filter_map(|node| match node {
                    Node::ToolResult { call_id, .. } => Some(call_id.clone()),
                    _ => None,
                })
                .collect();

            req.input.retain(|node| match node {
                Node::ToolCall { call_id, .. } => result_ids.contains(call_id),
                _ => true,
            });
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StripOrphanedToolUseTransform),
});

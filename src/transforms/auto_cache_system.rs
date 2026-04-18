use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, OrdinaryRole};
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

        let system_idx = req.input.iter().rposition(|node| {
            matches!(
                node.role(),
                Some(OrdinaryRole::System | OrdinaryRole::Developer)
            )
        });
        let Some(idx) = system_idx else {
            return Ok(());
        };

        let already_has_cache = node_has_cache_control(&req.input[idx]);
        if already_has_cache {
            return Ok(());
        }

        req.input[idx]
            .extra_body_mut()
            .insert("cache_control".to_string(), json!({"type": "ephemeral"}));

        Ok(())
    }
}

fn count_cache_breakpoints(req: &crate::urp::UrpRequest) -> usize {
    req.input
        .iter()
        .filter(|node| node_has_cache_control(node))
        .count()
}

fn node_has_cache_control(node: &Node) -> bool {
    match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body.contains_key("cache_control"),
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(AutoCacheSystemTransform),
});

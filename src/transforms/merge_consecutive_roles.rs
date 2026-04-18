use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::Node;
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

pub struct MergeConsecutiveRolesTransform;

#[async_trait]
impl Transform for MergeConsecutiveRolesTransform {
    fn type_id(&self) -> &'static str {
        "merge_consecutive_roles"
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
            req.input = merge_same_role_nodes(&req.input);
        }
        Ok(())
    }
}

fn merge_same_role_nodes(nodes: &[Node]) -> Vec<Node> {
    let mut merged: Vec<Node> = Vec::new();
    for node in nodes {
        if let (
            Some(Node::Text {
                role: last_role,
                content: last_content,
                phase: last_phase,
                extra_body: last_extra,
                ..
            }),
            Node::Text {
                role,
                content,
                phase,
                extra_body,
                ..
            },
        ) = (merged.last_mut(), node)
            && last_role == role
            && last_phase == phase
        {
            last_content.push_str(content);
            for (k, v) in extra_body {
                last_extra.entry(k.clone()).or_insert_with(|| v.clone());
            }
            continue;
        }
        merged.push(node.clone());
    }
    merged
}

inventory::submit!(TransformEntry {
    factory: || Box::new(MergeConsecutiveRolesTransform),
});

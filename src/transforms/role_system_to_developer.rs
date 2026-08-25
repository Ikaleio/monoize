use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
    move_system_to_developer_nodes,
};
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

pub struct RoleSystemToDeveloperTransform;

#[async_trait]
impl Transform for RoleSystemToDeveloperTransform {
    fn type_id(&self) -> &'static str {
        "role_system_to_developer"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[("en", "Role: system to developer"), ("zh", "角色：system 转 developer")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Rewrites system-role ordinary nodes to the developer role. Inverse of role_developer_to_system."),
            ("zh", "将 system 角色的普通节点改写为 developer 角色。与 role_developer_to_system 互逆。"),
        ]
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
            move_system_to_developer_nodes(&mut req.input);
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(RoleSystemToDeveloperTransform),
});

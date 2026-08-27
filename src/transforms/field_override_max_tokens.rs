use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformState, UrpData,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {
    value: u64,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct FieldOverrideMaxTokensTransform;

#[async_trait]
impl Transform for FieldOverrideMaxTokensTransform {
    fn type_id(&self) -> &'static str {
        "field_override_max_tokens"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Field: override max tokens"),
            ("zh", "字段：覆盖最大输出 token"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Sets the request max output token limit to the configured value.",
            ),
            ("zh", "将请求的最大输出 token 上限设置为配置值。"),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "integer", "minimum": 1 } },
            "required": ["value"],
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
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        if let UrpData::Request(req) = data {
            req.max_output_tokens = Some(cfg.value);
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(FieldOverrideMaxTokensTransform),
});

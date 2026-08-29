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
    enabled: bool,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct StreamForceTransform;

#[async_trait]
impl Transform for StreamForceTransform {
    fn type_id(&self) -> &'static str {
        "stream_force"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Stream: force streaming mode"),
            ("zh", "流式：强制流式模式"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Forces the upstream request stream flag to the configured enabled value.",
            ),
            (
                "zh",
                "将上游请求的 stream 标志强制设置为配置的 enabled 值。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "enabled": { "type": "boolean" } },
            "required": ["enabled"],
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
            req.stream = Some(cfg.enabled);
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StreamForceTransform),
});

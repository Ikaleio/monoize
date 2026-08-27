use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

pub const DEFAULT_MAX_FRAME_LENGTH: usize = 131_072;

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_max_frame_length")]
    max_frame_length: usize,
}

fn default_max_frame_length() -> usize {
    DEFAULT_MAX_FRAME_LENGTH
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct StreamSplitSseFramesTransform;

#[async_trait]
impl Transform for StreamSplitSseFramesTransform {
    fn type_id(&self) -> &'static str {
        "stream_split_sse_frames"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Stream: split oversized SSE frames"),
            ("zh", "流式：拆分超长 SSE 帧"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Splits oversized downstream SSE delta payloads into multiple protocol-valid events below the configured frame length.",
            ),
            (
                "zh",
                "将超过配置长度的下游 SSE 增量载荷拆分为多个协议合法的事件。",
            ),
        ]
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
            "properties": {
                "max_frame_length": {
                    "type": "integer",
                    "minimum": 1
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        if cfg.max_frame_length == 0 {
            return Err(TransformError::InvalidConfig(
                "max_frame_length must be >= 1".to_string(),
            ));
        }
        Ok(Box::new(cfg))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(NoState)
    }

    async fn apply(
        &self,
        _data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StreamSplitSseFramesTransform),
});

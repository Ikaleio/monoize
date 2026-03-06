use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformState,
    UrpData,
};
use async_trait::async_trait;
use crate::urp::{Message, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_content")]
    content: String,
}

fn default_content() -> String {
    " ".to_string()
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct AppendEmptyUserMessageTransform;

#[async_trait]
impl Transform for AppendEmptyUserMessageTransform {
    fn type_id(&self) -> &'static str {
        "append_empty_user_message"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "Text content for the padding user message. Defaults to a single space." }
            },
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
            if let Some(last) = req.messages.last() {
                if last.role == Role::Assistant {
                    req.messages.push(Message::text(Role::User, cfg.content.clone()));
                }
            }
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(AppendEmptyUserMessageTransform),
});

use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState,
    UrpData, text_part,
};
use crate::urp::{Message, Part, Role};
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Position {
    Prepend,
    Append,
}

#[derive(Debug, Deserialize)]
struct Config {
    content: String,
    position: Position,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct InjectSystemPromptTransform;

impl Transform for InjectSystemPromptTransform {
    fn type_id(&self) -> &'static str {
        "inject_system_prompt"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" },
                "position": { "type": "string", "enum": ["prepend", "append"] }
            },
            "required": ["content", "position"],
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

    fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        let mut target_index: Option<usize> = None;
        match cfg.position {
            Position::Prepend => {
                for (idx, msg) in req.messages.iter().enumerate() {
                    if msg.role == Role::System {
                        target_index = Some(idx);
                        break;
                    }
                }
            }
            Position::Append => {
                for (idx, msg) in req.messages.iter().enumerate().rev() {
                    if msg.role == Role::System {
                        target_index = Some(idx);
                        break;
                    }
                }
            }
        }

        if let Some(idx) = target_index {
            req.messages[idx].parts.push(Part::Text {
                content: cfg.content.clone(),
                extra_body: std::collections::HashMap::new(),
            });
        } else {
            let mut message = Message::new(Role::System);
            message.parts.push(text_part(cfg.content.clone()));
            match cfg.position {
                Position::Prepend => req.messages.insert(0, message),
                Position::Append => req.messages.push(message),
            }
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(InjectSystemPromptTransform),
});

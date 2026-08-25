use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData, text_node,
};
use crate::urp::OrdinaryRole;
use async_trait::async_trait;
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

pub struct PromptInjectSystemTransform;

#[async_trait]
impl Transform for PromptInjectSystemTransform {
    fn type_id(&self) -> &'static str {
        "prompt_inject_system"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[("en", "Prompt: inject system prompt"), ("zh", "提示词：注入系统提示词")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Prepends or appends the configured text to the request system prompt, creating a system node when none exists."),
            ("zh", "将配置文本前置或追加到系统提示词；不存在 system 节点时创建新节点。"),
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
            "properties": {
                "content": { "type": "string", "format": "multiline" },
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
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        let mut target_index: Option<usize> = None;
        match cfg.position {
            Position::Prepend => {
                for (idx, node) in req.input.iter().enumerate() {
                    if matches!(node.role(), Some(OrdinaryRole::System)) {
                        target_index = Some(idx);
                        break;
                    }
                }
            }
            Position::Append => {
                for (idx, node) in req.input.iter().enumerate().rev() {
                    if matches!(node.role(), Some(OrdinaryRole::System)) {
                        target_index = Some(idx);
                        break;
                    }
                }
            }
        }

        if let Some(idx) = target_index {
            req.input.insert(
                idx + 1,
                text_node(OrdinaryRole::System, cfg.content.clone()),
            );
        } else {
            let message = text_node(OrdinaryRole::System, cfg.content.clone());
            match cfg.position {
                Position::Prepend => req.input.insert(0, message),
                Position::Append => req.input.push(message),
            }
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(PromptInjectSystemTransform),
});

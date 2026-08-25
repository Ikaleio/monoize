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

pub struct PromptAppendEmptyUserTransform;

#[async_trait]
impl Transform for PromptAppendEmptyUserTransform {
    fn type_id(&self) -> &'static str {
        "prompt_append_empty_user"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[("en", "Prompt: append empty user message"), ("zh", "提示词：追加空 user 消息")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Appends a padding user text node when the request input ends with an assistant node."),
            ("zh", "当请求输入以 assistant 节点结尾时，追加一个占位 user 文本节点。"),
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
                "content": { "type": "string", "format": "multiline", "description": "Text content for the padding user message. Defaults to a single space." }
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
            if req
                .input
                .last()
                .is_some_and(|node| node.role() == Some(OrdinaryRole::Assistant))
            {
                req.input.push(Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: cfg.content.clone(),
                    phase: None,
                    extra_body: std::collections::HashMap::new(),
                });
            }
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(PromptAppendEmptyUserTransform),
});

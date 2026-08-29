use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{ToolChoice, ToolDefinition};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

const FORCE_STREAM_PARTIAL_IMAGES: u64 = 3;

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default = "default_output_format")]
    output_format: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    force_stream: bool,
    #[serde(default)]
    force_tool_choice: bool,
    #[serde(default)]
    extra: HashMap<String, Value>,
}

fn default_output_format() -> String {
    "png".to_string()
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ImageEnableOpenAiGenerationToolTransform;

#[async_trait]
impl Transform for ImageEnableOpenAiGenerationToolTransform {
    fn type_id(&self) -> &'static str {
        "image_enable_openai_generation_tool"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Image: enable OpenAI generation tool"),
            ("zh", "图像：启用 OpenAI 生成工具"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Ensures the OpenAI Responses image_generation tool descriptor exists on the request, optionally forcing streaming and tool choice.",
            ),
            (
                "zh",
                "确保请求携带 OpenAI Responses image_generation 工具描述，可选强制流式与 tool_choice。",
            ),
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
                "output_format": {
                    "type": "string",
                    "enum": ["png", "webp", "jpeg"],
                    "default": "png"
                },
                "action": {
                    "type": "string",
                    "minLength": 1
                },
                "force_stream": {
                    "type": "boolean",
                    "default": false
                },
                "force_tool_choice": {
                    "type": "boolean",
                    "default": false
                },
                "extra": {
                    "type": "object",
                    "default": {}
                }
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
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?
            .clone();
        let UrpData::Request(req) = data else {
            return Ok(());
        };
        if cfg.force_stream {
            req.stream = Some(true);
        }
        if cfg.force_tool_choice {
            req.tool_choice = Some(ToolChoice::Specific(json!({
                "type": "image_generation"
            })));
        }

        let tools = req.tools.get_or_insert_with(Vec::new);
        if cfg.force_stream {
            let mut found_existing = false;
            for tool in tools
                .iter_mut()
                .filter(|tool| tool.tool_type == "image_generation")
            {
                tool.extra_body.insert(
                    "partial_images".to_string(),
                    Value::from(FORCE_STREAM_PARTIAL_IMAGES),
                );
                found_existing = true;
            }
            if found_existing {
                return Ok(());
            }
        } else if tools
            .iter()
            .any(|tool| tool.tool_type == "image_generation")
        {
            return Ok(());
        }

        let mut extra_body = HashMap::new();
        for key in ["size", "quality"] {
            if let Some(value) = req.extra_body.get(key) {
                extra_body.insert(key.to_string(), value.clone());
            }
        }
        extra_body.extend(cfg.extra.clone());
        extra_body.insert(
            "output_format".to_string(),
            Value::String(cfg.output_format.clone()),
        );
        if let Some(action) = cfg.action.filter(|value| !value.is_empty()) {
            extra_body.insert("action".to_string(), Value::String(action));
        }
        if cfg.force_stream {
            extra_body.insert(
                "partial_images".to_string(),
                Value::from(FORCE_STREAM_PARTIAL_IMAGES),
            );
        }
        tools.push(ToolDefinition {
            tool_type: "image_generation".to_string(),
            name: None,
            description: None,
            function: None,
            custom: None,
            extra_body,
        });
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ImageEnableOpenAiGenerationToolTransform),
});

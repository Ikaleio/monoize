use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{ImageGenerationOptions, ToolChoice, ToolDefinition};
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
        let mut legacy_image_options = HashMap::new();
        for key in ImageGenerationOptions::KEYS {
            if key == "background" && req.extra_body.get(key).is_some_and(Value::is_boolean) {
                continue;
            }
            if let Some(value) = req.extra_body.remove(key) {
                legacy_image_options.insert(key.to_owned(), value);
            }
        }
        if req.image_generation.is_none() && !legacy_image_options.is_empty() {
            req.image_generation = Some(
                ImageGenerationOptions::take_from_extra(&mut legacy_image_options)
                    .map_err(TransformError::Apply)?,
            );
        }
        if cfg.force_stream {
            req.stream = Some(true);
        }
        if cfg.force_tool_choice {
            req.tool_choice = Some(ToolChoice::Specific(json!({
                "type": "image_generation"
            })));
        }

        let default_partial_images = cfg.force_stream
            && !req
                .image_generation
                .as_ref()
                .is_some_and(|options| options.partial_images.is_some());
        let tools = req.tools.get_or_insert_with(Vec::new);
        let mut found_existing = false;
        for tool in tools
            .iter_mut()
            .filter(|tool| tool.tool_type == "image_generation")
        {
            found_existing = true;
            if default_partial_images
                && !tool.extra_body.contains_key("partial_images")
                && !tool
                    .config
                    .as_ref()
                    .is_some_and(|config| config.get("partial_images").is_some())
            {
                let config = tool.config.get_or_insert_with(|| json!({}));
                let config = config.as_object_mut().ok_or_else(|| {
                    TransformError::Apply("Image generation tool config must be an object.".into())
                })?;
                config.insert(
                    "partial_images".to_string(),
                    Value::from(FORCE_STREAM_PARTIAL_IMAGES),
                );
            }
        }
        if found_existing {
            return Ok(());
        }

        let mut tool_config: serde_json::Map<String, Value> = cfg.extra.into_iter().collect();
        tool_config.insert(
            "output_format".to_string(),
            Value::String(cfg.output_format),
        );
        if let Some(action) = cfg.action.filter(|value| !value.is_empty()) {
            tool_config.insert("action".to_string(), Value::String(action));
        }
        if default_partial_images {
            tool_config
                .entry("partial_images".to_string())
                .or_insert(Value::from(FORCE_STREAM_PARTIAL_IMAGES));
        }
        tools.push(ToolDefinition {
            namespace: None,
            tools: None,
            origin_protocol: None,
            config: Some(Value::Object(tool_config)),

            tool_type: "image_generation".to_string(),
            name: None,
            description: None,
            function: None,
            custom: None,
            extra_body: HashMap::new(),
        });
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ImageEnableOpenAiGenerationToolTransform),
});

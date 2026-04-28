use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::ToolDefinition;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default = "default_output_format")]
    output_format: String,
    #[serde(default)]
    action: Option<String>,
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

pub struct EnableOpenAiImageGenerationToolTransform;

#[async_trait]
impl Transform for EnableOpenAiImageGenerationToolTransform {
    fn type_id(&self) -> &'static str {
        "enable_openai_image_generation_tool"
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

        let tools = req.tools.get_or_insert_with(Vec::new);
        if tools
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
        tools.push(ToolDefinition {
            tool_type: "image_generation".to_string(),
            function: None,
            extra_body,
        });
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(EnableOpenAiImageGenerationToolTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::TransformRuntimeContext;
    use crate::urp::UrpRequest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn context() -> TransformRuntimeContext {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("cache");
        TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
        }
    }

    #[tokio::test]
    async fn appends_image_generation_tool_when_missing() {
        let transform = EnableOpenAiImageGenerationToolTransform;
        let config = transform
            .parse_config(json!({ "output_format": "png" }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(Vec::new()),
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                config.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let tools = req.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_type, "image_generation");
        assert_eq!(
            tools[0].extra_body.get("output_format"),
            Some(&json!("png"))
        );
    }

    #[tokio::test]
    async fn leaves_existing_image_generation_tool_unchanged() {
        let transform = EnableOpenAiImageGenerationToolTransform;
        let config = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(vec![ToolDefinition {
                tool_type: "image_generation".to_string(),
                function: None,
                extra_body: HashMap::from([(
                    "output_format".to_string(),
                    Value::String("webp".to_string()),
                )]),
            }]),
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                config.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let tools = req.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].extra_body.get("output_format"),
            Some(&json!("webp"))
        );
    }

    #[tokio::test]
    async fn injects_arbitrary_extra_fields_into_image_generation_tool() {
        let transform = EnableOpenAiImageGenerationToolTransform;
        let config = transform
            .parse_config(json!({
                "output_format": "png",
                "extra": {
                    "quality": "high",
                    "size": "1024x1024",
                    "background": "transparent"
                }
            }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(Vec::new()),
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                config.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let tools = req.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].extra_body.get("quality"), Some(&json!("high")));
        assert_eq!(tools[0].extra_body.get("size"), Some(&json!("1024x1024")));
        assert_eq!(
            tools[0].extra_body.get("background"),
            Some(&json!("transparent"))
        );
        assert_eq!(
            tools[0].extra_body.get("output_format"),
            Some(&json!("png"))
        );
    }

    #[tokio::test]
    async fn promotes_root_size_and_quality_into_image_generation_tool() {
        let transform = EnableOpenAiImageGenerationToolTransform;
        let config = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(Vec::new()),
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::from([
                ("size".to_string(), json!("1280x720")),
                ("quality".to_string(), json!("high")),
                ("background".to_string(), json!("transparent")),
            ]),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                config.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let tools = req.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].extra_body.get("size"), Some(&json!("1280x720")));
        assert_eq!(tools[0].extra_body.get("quality"), Some(&json!("high")));
        assert_eq!(tools[0].extra_body.get("background"), None);
    }

    #[tokio::test]
    async fn extra_size_and_quality_override_promoted_root_fields() {
        let transform = EnableOpenAiImageGenerationToolTransform;
        let config = transform
            .parse_config(json!({
                "extra": {
                    "size": "1024x1024",
                    "quality": "low"
                }
            }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(Vec::new()),
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::from([
                ("size".to_string(), json!("1280x720")),
                ("quality".to_string(), json!("high")),
            ]),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                config.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let tools = req.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].extra_body.get("size"), Some(&json!("1024x1024")));
        assert_eq!(tools[0].extra_body.get("quality"), Some(&json!("low")));
    }

    #[tokio::test]
    async fn explicit_fields_override_conflicting_extra_entries() {
        let transform = EnableOpenAiImageGenerationToolTransform;
        let config = transform
            .parse_config(json!({
                "output_format": "jpeg",
                "action": "edit",
                "extra": {
                    "output_format": "webp",
                    "action": "generate",
                    "quality": "high"
                }
            }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-5.4".to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(Vec::new()),
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                config.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let tools = req.tools.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].extra_body.get("quality"), Some(&json!("high")));
        assert_eq!(
            tools[0].extra_body.get("output_format"),
            Some(&json!("jpeg"))
        );
        assert_eq!(tools[0].extra_body.get("action"), Some(&json!("edit")));
    }
}

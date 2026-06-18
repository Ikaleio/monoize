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

pub struct ForceStreamTransform;

#[async_trait]
impl Transform for ForceStreamTransform {
    fn type_id(&self) -> &'static str {
        "force_stream"
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
    factory: || Box::new(ForceStreamTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::TransformRuntimeContext;
    use crate::urp::{Node, OrdinaryRole, UrpRequest};
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
            upstream_provider_type: None,
        }
    }

    fn request(stream: Option<bool>) -> UrpRequest {
        UrpRequest {
            model: "gpt-image-1".to_string(),
            input: vec![Node::Text {
                id: None,
                role: OrdinaryRole::User,
                content: "draw a cat".to_string(),
                phase: None,
                extra_body: HashMap::new(),
            }],
            stream,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn enabled_sets_request_stream_true_for_image_upstream_collection() {
        let transform = ForceStreamTransform;
        let config = transform
            .parse_config(json!({ "enabled": true }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = request(Some(false));

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

        assert_eq!(req.stream, Some(true));
    }

    #[tokio::test]
    async fn disabled_sets_request_stream_false() {
        let transform = ForceStreamTransform;
        let config = transform
            .parse_config(json!({ "enabled": false }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = request(Some(true));

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

        assert_eq!(req.stream, Some(false));
    }
}

use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, OrdinaryRole};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

const HEADER_PREFIX: &str = "x-anthropic-billing-header:";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct StripAnthropicBillingHeaderTransform;

#[async_trait]
impl Transform for StripAnthropicBillingHeaderTransform {
    fn type_id(&self) -> &'static str {
        "strip_anthropic_billing_header"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[
            TransformScope::Provider,
            TransformScope::Global,
            TransformScope::ApiKey,
        ]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
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
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        req.input.retain_mut(|node| match node {
            Node::Text { role, content, .. }
                if matches!(role, OrdinaryRole::System | OrdinaryRole::Developer) =>
            {
                strip_header_lines(content);
                !content.is_empty()
            }
            _ => true,
        });

        Ok(())
    }
}

fn strip_header_lines(content: &mut String) {
    let mut out = String::new();
    for segment in content.split_inclusive('\n') {
        let line = segment.trim_end_matches('\n').trim_end_matches('\r');
        if line.trim_start().starts_with(HEADER_PREFIX) {
            continue;
        }
        out.push_str(segment);
    }
    if !content.ends_with('\n') && content.lines().count() <= 1 {
        let line = content.trim_end_matches('\r');
        if line.trim_start().starts_with(HEADER_PREFIX) {
            out.clear();
        }
    }
    *content = out.trim_matches('\n').trim_end_matches('\r').to_string();
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StripAnthropicBillingHeaderTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::UrpData;
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
            upstream_provider_type: None,
        }
    }

    #[tokio::test]
    async fn strips_billing_header_from_system_text_nodes() {
        let transform = StripAnthropicBillingHeaderTransform;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-test".to_string(),
            input: vec![
                Node::text(
                    OrdinaryRole::System,
                    "x-anthropic-billing-header: cc_version=1; cch=abc;\nKeep this.",
                ),
                Node::text(
                    OrdinaryRole::User,
                    "x-anthropic-billing-header: this user text must remain;",
                ),
            ],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        assert_eq!(req.input[0], Node::text(OrdinaryRole::System, "Keep this."));
        assert_eq!(
            req.input[1],
            Node::text(
                OrdinaryRole::User,
                "x-anthropic-billing-header: this user text must remain;"
            )
        );
    }

    #[tokio::test]
    async fn removes_empty_system_node_after_stripping() {
        let transform = StripAnthropicBillingHeaderTransform;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut req = UrpRequest {
            model: "gpt-test".to_string(),
            input: vec![Node::text(
                OrdinaryRole::System,
                "x-anthropic-billing-header: cc_version=1; cch=abc;",
            )],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context().await,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        assert!(req.input.is_empty());
    }
}

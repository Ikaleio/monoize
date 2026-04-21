use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct NoOpState;

impl TransformState for NoOpState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ReasoningSummaryToRawCotTransform;

#[async_trait]
impl Transform for ReasoningSummaryToRawCotTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_summary_to_raw_cot"
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
        Box::new(NoOpState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        match data {
            UrpData::Response(resp) => {
                for node in &mut resp.output {
                    mark_node(node);
                }
            }
            UrpData::Stream(event) => mark_stream(event),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn mark_node(node: &mut Node) {
    let Node::Reasoning {
        summary,
        extra_body,
        ..
    } = node
    else {
        return;
    };
    if summary
        .as_deref()
        .is_some_and(|summary| !summary.is_empty())
    {
        extra_body.insert("openwebui_reasoning_content".to_string(), Value::Bool(true));
    }
}

fn mark_stream(event: &mut UrpStreamEvent) {
    match event {
        UrpStreamEvent::NodeDelta {
            delta, extra_body, ..
        } => {
            if let NodeDelta::Reasoning { summary, .. } = delta
                && summary
                    .as_deref()
                    .is_some_and(|summary| !summary.is_empty())
            {
                extra_body.insert("openwebui_reasoning_content".to_string(), Value::Bool(true));
            }
        }
        UrpStreamEvent::NodeDone { node, .. } => mark_node(node),
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                mark_node(node);
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningSummaryToRawCotTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::{TransformRuntimeContext, build_states_for_rules, registry};
    use crate::urp::UrpResponse;
    use crate::urp::internal_legacy_bridge::{Item, Part, Role, items_to_nodes, nodes_to_items};
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
    async fn marks_summary_reasoning_parts_for_openwebui_raw_cot() {
        let registry = registry();
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "reasoning_summary_to_raw_cot".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Response,
            config: json!({}),
        }];
        let mut states = build_states_for_rules(&rules, &registry).expect("states");
        let mut resp = UrpResponse {
            id: "resp_1".to_string(),
            model: "gpt-test".to_string(),
            created_at: None,
            output: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    id: None,
                    content: Some("full reasoning".to_string()),
                    encrypted: None,
                    summary: Some("brief summary".to_string()),
                    source: None,
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            }]),
            finish_reason: None,
            usage: None,
            extra_body: HashMap::new(),
        };
        crate::transforms::apply_transforms(
            UrpData::Response(&mut resp),
            &rules,
            &mut states,
            "gpt-test",
            Phase::Response,
            &context().await,
            &registry,
        )
        .await
        .expect("apply");

        let outputs = nodes_to_items(&resp.output);
        let Item::Message { parts, .. } = &outputs[0] else {
            panic!("expected message");
        };
        let Part::Reasoning { extra_body, .. } = &parts[0] else {
            panic!("expected reasoning");
        };
        assert_eq!(
            extra_body
                .get("openwebui_reasoning_content")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn marks_summary_reasoning_stream_deltas_for_openwebui_raw_cot() {
        let transform = ReasoningSummaryToRawCotTransform;
        let context = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut event = UrpStreamEvent::NodeDelta {
            node_index: 7,
            delta: NodeDelta::Reasoning {
                content: None,
                encrypted: None,
                summary: Some("brief summary".to_string()),
                source: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        };
        transform
            .apply(
                UrpData::Stream(&mut event),
                Phase::Response,
                &context,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let UrpStreamEvent::NodeDelta { extra_body, .. } = event else {
            panic!("expected delta");
        };
        assert_eq!(
            extra_body
                .get("openwebui_reasoning_content")
                .and_then(Value::as_bool),
            Some(true)
        );
    }
}

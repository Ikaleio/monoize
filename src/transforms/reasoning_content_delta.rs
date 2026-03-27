use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData, response_output_items_mut,
};
use crate::urp::{Item, Part, PartDelta, UrpStreamEvent};
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

pub struct ReasoningContentDeltaTransform;

#[async_trait]
impl Transform for ReasoningContentDeltaTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_content_delta"
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
                for item in response_output_items_mut(resp) {
                    mark_item(item);
                }
            }
            UrpData::Stream(event) => mark_stream(event),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn extract_reasoning_content(content: &Option<String>, summary: &Option<String>) -> Option<String> {
    if let Some(content) = content {
        if !content.is_empty() {
            return Some(content.clone());
        }
    }
    if let Some(sum) = summary {
        if !sum.is_empty() {
            return Some(sum.clone());
        }
    }
    None
}

fn mark_item(item: &mut Item) {
    let Item::Message { parts, .. } = item else {
        return;
    };
    for part in parts {
        let Part::Reasoning {
            content,
            encrypted,
            summary,
            extra_body,
            ..
        } = part
        else {
            continue;
        };
        let _ = encrypted;
        if let Some(value) = extract_reasoning_content(content, summary) {
            extra_body.insert("inject_reasoning_content".to_string(), Value::String(value));
        }
    }
}

fn mark_stream(event: &mut UrpStreamEvent) {
    match event {
        UrpStreamEvent::Delta {
            delta:
                PartDelta::Reasoning {
                    content,
                    encrypted,
                    summary,
                    ..
                },
            extra_body,
            ..
        } => {
            let _ = encrypted;
            if let Some(value) = extract_reasoning_content(content, summary) {
                extra_body.insert("inject_reasoning_content".to_string(), Value::String(value));
            }
        }
        UrpStreamEvent::PartDone { part, .. } => {
            let Part::Reasoning {
                content,
                encrypted,
                summary,
                extra_body,
                ..
            } = part
            else {
                return;
            };
            let _ = encrypted;
            if let Some(value) = extract_reasoning_content(content, summary) {
                extra_body.insert("inject_reasoning_content".to_string(), Value::String(value));
            }
        }
        UrpStreamEvent::ItemDone { item, .. } => mark_item(item),
        UrpStreamEvent::ResponseDone { outputs, .. } => {
            for item in outputs {
                mark_item(item);
            }
        }
        _ => {}
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningContentDeltaTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::TransformRuntimeContext;
    use crate::urp::PartDelta;
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
    async fn injects_plaintext_reasoning_content_when_present() {
        let transform = ReasoningContentDeltaTransform;
        let ctx = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut event = UrpStreamEvent::Delta {
            part_index: 0,
            delta: PartDelta::Reasoning {
                content: Some("plaintext_reasoning".to_string()),
                encrypted: Some(Value::String("encrypted_data".to_string())),
                summary: Some("summary_text".to_string()),
                source: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        };
        transform
            .apply(
                UrpData::Stream(&mut event),
                Phase::Response,
                &ctx,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let UrpStreamEvent::Delta { extra_body, .. } = event else {
            panic!("expected delta");
        };
        assert_eq!(
            extra_body
                .get("inject_reasoning_content")
                .and_then(Value::as_str),
            Some("plaintext_reasoning"),
            "should prefer plaintext content over summary and ignore encrypted"
        );
    }

    #[tokio::test]
    async fn falls_back_to_summary_when_no_plaintext() {
        let transform = ReasoningContentDeltaTransform;
        let ctx = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut event = UrpStreamEvent::Delta {
            part_index: 0,
            delta: PartDelta::Reasoning {
                content: None,
                encrypted: None,
                summary: Some("summary_fallback".to_string()),
                source: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        };
        transform
            .apply(
                UrpData::Stream(&mut event),
                Phase::Response,
                &ctx,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let UrpStreamEvent::Delta { extra_body, .. } = event else {
            panic!("expected delta");
        };
        assert_eq!(
            extra_body
                .get("inject_reasoning_content")
                .and_then(Value::as_str),
            Some("summary_fallback"),
            "should fall back to summary when plaintext content is absent"
        );
    }

    #[tokio::test]
    async fn does_not_inject_when_only_encrypted_reasoning_exists() {
        let transform = ReasoningContentDeltaTransform;
        let ctx = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut event = UrpStreamEvent::Delta {
            part_index: 0,
            delta: PartDelta::Reasoning {
                content: None,
                encrypted: Some(Value::String("encrypted_only".to_string())),
                summary: None,
                source: None,
            },
            usage: None,
            extra_body: HashMap::new(),
        };
        transform
            .apply(
                UrpData::Stream(&mut event),
                Phase::Response,
                &ctx,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let UrpStreamEvent::Delta { extra_body, .. } = event else {
            panic!("expected delta");
        };
        assert!(
            !extra_body.contains_key("inject_reasoning_content"),
            "should not inject encrypted-only reasoning content"
        );
    }

    #[tokio::test]
    async fn marks_response_parts_with_plaintext_reasoning() {
        let transform = ReasoningContentDeltaTransform;
        let ctx = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut resp = crate::urp::UrpResponse {
            id: "resp_1".to_string(),
            model: "test".to_string(),
            outputs: vec![Item::Message {
                role: crate::urp::Role::Assistant,
                parts: vec![Part::Reasoning {
                    content: Some("plain_resp".to_string()),
                    encrypted: Some(Value::String("enc_resp".to_string())),
                    summary: Some("sum_resp".to_string()),
                    source: None,
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            }],
            finish_reason: None,
            usage: None,
            extra_body: HashMap::new(),
        };
        transform
            .apply(
                UrpData::Response(&mut resp),
                Phase::Response,
                &ctx,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        let Item::Message { parts, .. } = &resp.outputs[0] else {
            panic!("expected message");
        };
        let Part::Reasoning { extra_body, .. } = &parts[0] else {
            panic!("expected reasoning");
        };
        assert_eq!(
            extra_body
                .get("inject_reasoning_content")
                .and_then(Value::as_str),
            Some("plain_resp"),
        );
    }
}

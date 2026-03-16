use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData, response_output_items_mut,
};
use crate::urp::{Item, Part, PartDelta, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct StreamState {
    encrypted_parts: HashSet<u32>,
}

impl TransformState for StreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct PlaintextReasoningToSummaryTransform;

#[async_trait]
impl Transform for PlaintextReasoningToSummaryTransform {
    fn type_id(&self) -> &'static str {
        "plaintext_reasoning_to_summary"
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
        let cfg: Config =
            serde_json::from_value(raw).map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        Ok(Box::new(cfg))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(StreamState::default())
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        match data {
            UrpData::Response(resp) => {
                for item in response_output_items_mut(resp) {
                    rewrite_item_reasoning(item);
                }
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<StreamState>() else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                rewrite_stream_reasoning(event, stream_state);
            }
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn rewrite_stream_reasoning(event: &mut UrpStreamEvent, state: &mut StreamState) {
    match event {
        UrpStreamEvent::Delta {
            part_index,
            delta,
            extra_body,
            ..
        } => {
            if !matches!(delta, PartDelta::Reasoning { .. }) {
                return;
            }
            if extra_body.contains_key("signature") {
                state.encrypted_parts.insert(*part_index);
                return;
            }
            if !state.encrypted_parts.contains(part_index) {
                extra_body.insert(
                    "reasoning_delta_type".to_string(),
                    Value::String("summary".to_string()),
                );
            }
        }
        UrpStreamEvent::PartDone {
            part_index, part, ..
        } => {
            if let Part::Reasoning { encrypted, .. } = part {
                if encrypted.is_some() {
                    state.encrypted_parts.insert(*part_index);
                }
            }
            rewrite_reasoning_part(part);
        }
        UrpStreamEvent::ItemDone { item, .. } => rewrite_item_reasoning(item),
        UrpStreamEvent::ResponseDone { outputs, .. } => {
            for item in outputs {
                rewrite_item_reasoning(item);
            }
        }
        _ => {}
    }
}

fn rewrite_item_reasoning(item: &mut Item) {
    let Item::Message { parts, .. } = item else {
        return;
    };
    for part in parts.iter_mut() {
        rewrite_reasoning_part(part);
    }
}

fn rewrite_reasoning_part(part: &mut Part) {
    let Part::Reasoning {
        content,
        summary,
        ..
    } = part
    else {
        return;
    };
    let Some(text) = content.take() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    *summary = Some(text);
}

inventory::submit!(TransformEntry {
    factory: || Box::new(PlaintextReasoningToSummaryTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::{TransformRuntimeContext, build_states_for_rules, registry};
    use crate::urp::{Role, UrpResponse};
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
        }
    }

    #[tokio::test]
    async fn moves_plaintext_reasoning_to_summary_in_response() {
        let registry = registry();
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "plaintext_reasoning_to_summary".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Response,
            config: json!({}),
        }];
        let mut states = build_states_for_rules(&rules, &registry).expect("states");
        let mut resp = UrpResponse {
            id: "resp_1".to_string(),
            model: "gpt-test".to_string(),
            outputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    content: Some("plain reasoning".to_string()),
                    encrypted: None,
                    summary: None,
                    source: None,
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            }],
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

        let Item::Message { parts, .. } = &resp.outputs[0] else {
            panic!("expected message");
        };
        let Part::Reasoning {
            content, summary, ..
        } = &parts[0]
        else {
            panic!("expected reasoning");
        };
        assert_eq!(content, &None);
        assert_eq!(summary.as_deref(), Some("plain reasoning"));
    }

    #[tokio::test]
    async fn preserves_encrypted_reasoning_while_summarizing_plaintext_in_response() {
        let registry = registry();
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "plaintext_reasoning_to_summary".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Response,
            config: json!({}),
        }];
        let mut states = build_states_for_rules(&rules, &registry).expect("states");
        let mut resp = UrpResponse {
            id: "resp_1".to_string(),
            model: "gpt-test".to_string(),
            outputs: vec![Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Reasoning {
                    content: Some("plain reasoning".to_string()),
                    encrypted: Some(Value::String("ciphertext".to_string())),
                    summary: None,
                    source: Some("openrouter".to_string()),
                    extra_body: HashMap::from([(
                        "preserved".to_string(),
                        Value::String("yes".to_string()),
                    )]),
                }],
                extra_body: HashMap::new(),
            }],
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

        let Item::Message { parts, .. } = &resp.outputs[0] else {
            panic!("expected message");
        };
        let Part::Reasoning {
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } = &parts[0]
        else {
            panic!("expected reasoning");
        };
        assert_eq!(content, &None);
        assert_eq!(summary.as_deref(), Some("plain reasoning"));
        assert_eq!(encrypted, &Some(Value::String("ciphertext".to_string())));
        assert_eq!(source.as_deref(), Some("openrouter"));
        assert_eq!(
            extra_body.get("preserved").and_then(Value::as_str),
            Some("yes")
        );
    }

    #[tokio::test]
    async fn marks_stream_reasoning_delta_as_summary_when_not_encrypted() {
        let transform = PlaintextReasoningToSummaryTransform;
        let context = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut event = UrpStreamEvent::Delta {
            part_index: 7,
            delta: PartDelta::Reasoning {
                content: "plain".to_string(),
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

        let UrpStreamEvent::Delta { extra_body, .. } = event else {
            panic!("expected delta");
        };
        assert_eq!(
            extra_body.get("reasoning_delta_type").and_then(Value::as_str),
            Some("summary")
        );
    }

    #[tokio::test]
    async fn preserves_encrypted_reasoning_while_summarizing_stream_part_done() {
        let transform = PlaintextReasoningToSummaryTransform;
        let context = context().await;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut event = UrpStreamEvent::PartDone {
            part_index: 2,
            part: Part::Reasoning {
                content: Some("plain".to_string()),
                encrypted: Some(Value::String("ciphertext".to_string())),
                summary: None,
                source: Some("openrouter".to_string()),
                extra_body: HashMap::from([(
                    "preserved".to_string(),
                    Value::String("yes".to_string()),
                )]),
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

        let UrpStreamEvent::PartDone { part, .. } = event else {
            panic!("expected part done");
        };
        let Part::Reasoning {
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } = part
        else {
            panic!("expected reasoning");
        };
        assert_eq!(content, None);
        assert_eq!(summary.as_deref(), Some("plain"));
        assert_eq!(encrypted, Some(Value::String("ciphertext".to_string())));
        assert_eq!(source.as_deref(), Some("openrouter"));
        assert_eq!(
            extra_body.get("preserved").and_then(Value::as_str),
            Some("yes")
        );
    }
}

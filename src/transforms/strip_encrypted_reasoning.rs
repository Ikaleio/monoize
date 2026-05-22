//! `strip_encrypted_reasoning` response-phase transform.
//!
//! Drops opaque `encrypted` reasoning payloads from `Reasoning` nodes,
//! reasoning deltas, and reasoning-bearing envelope-extra control events.
//! Plaintext reasoning surfaces (`content`, `summary`, `source`) and node-local
//! `extra_body` keys other than `encrypted_content` are preserved.
//!
//! This transform exists to mitigate downstream SSE clients that cannot read
//! single SSE `data:` lines exceeding their per-line buffer (commonly 128 KiB,
//! e.g. OpenWebUI / aiohttp). For long reasoning, an `mz2.` envelope payload
//! plus the surrounding Responses `response.completed` JSON object easily
//! exceeds that limit. Stripping `encrypted_content` shrinks the per-line
//! payload while keeping the rest of the response semantically intact.
//!
//! Per `spec/urp-transform-system.spec.md` PIPE-1d and `spec/unified_responses_proxy.spec.md`
//! PR4c.3, when `reasoning_envelope_enabled = true` the runtime wraps any
//! upstream-produced encrypted reasoning into `mz2.` envelopes before
//! response-phase transforms observe the response. When enabled, this
//! transform observes the envelope form and strips it; when
//! `reasoning_envelope_enabled = false` it observes the raw upstream value
//! and strips that instead. The transform is agnostic to whether the value is
//! still envelope-wrapped.

use crate::transforms::{
    Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, NodeDelta, NodeHeader, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct StripEncryptedReasoningTransform;

#[async_trait]
impl Transform for StripEncryptedReasoningTransform {
    fn type_id(&self) -> &'static str {
        "strip_encrypted_reasoning"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
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
        Box::new(crate::transforms::NoState)
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
                for node in resp.output.iter_mut() {
                    strip_encrypted_in_node(node);
                }
            }
            UrpData::Stream(event) => strip_encrypted_in_stream_event(event),
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn strip_encrypted_in_node(node: &mut Node) {
    match node {
        Node::Reasoning {
            encrypted,
            extra_body,
            ..
        } => {
            *encrypted = None;
            extra_body.remove("encrypted_content");
        }
        Node::NextDownstreamEnvelopeExtra { extra_body } if envelope_is_reasoning(extra_body) => {
            extra_body.remove("encrypted_content");
        }
        _ => {}
    }
}

fn strip_encrypted_in_stream_event(event: &mut UrpStreamEvent) {
    match event {
        UrpStreamEvent::NodeStart {
            header: NodeHeader::Reasoning { .. },
            extra_body,
            ..
        } => {
            extra_body.remove("encrypted_content");
        }
        UrpStreamEvent::NodeStart {
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            extra_body,
            ..
        } if envelope_is_reasoning(extra_body) => {
            extra_body.remove("encrypted_content");
        }
        UrpStreamEvent::NodeDelta {
            delta: NodeDelta::Reasoning { encrypted, .. },
            ..
        } => {
            *encrypted = None;
        }
        UrpStreamEvent::NodeDone { node, .. } => {
            strip_encrypted_in_node(node);
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output.iter_mut() {
                strip_encrypted_in_node(node);
            }
        }
        _ => {}
    }
}

/// Mirror of `urp::extra_body_is_reasoning_item`: detect whether a control-node
/// envelope-extra carries reasoning-item state (encrypted_content present, or
/// `type = "reasoning"`). Kept local to avoid widening the public surface of
/// `urp::mod`.
fn envelope_is_reasoning(extra_body: &HashMap<String, Value>) -> bool {
    extra_body.contains_key("encrypted_content")
        || extra_body.get("type").and_then(Value::as_str) == Some("reasoning")
}

inventory::submit!(TransformEntry {
    factory: || Box::new(StripEncryptedReasoningTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::{NoState, TransformRuntimeContext};
    use crate::urp::{Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn ctx() -> (TempDir, TransformRuntimeContext) {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("cache");
        let context = TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
        };
        (temp_dir, context)
    }

    #[tokio::test]
    async fn strips_encrypted_from_reasoning_node_in_response() {
        let transform = StripEncryptedReasoningTransform;
        let cfg = transform.parse_config(json!({})).unwrap();
        let mut state = transform.init_state();
        let (_tmp, context) = ctx().await;
        let mut resp = crate::urp::UrpResponse {
            id: "resp_1".into(),
            model: "m".into(),
            created_at: None,
            output: vec![
                Node::Reasoning {
                    id: Some("rs_1".into()),
                    content: Some("plaintext cot".into()),
                    encrypted: Some(json!("mz2.aaaaaaaa")),
                    summary: Some("summary".into()),
                    source: None,
                    extra_body: [("encrypted_content".to_string(), json!("mz2.bbbbbbbb"))]
                        .into_iter()
                        .collect(),
                },
                Node::assistant_text("hi"),
            ],
            finish_reason: None,
            usage: None,
            extra_body: HashMap::new(),
        };

        transform
            .apply(
                UrpData::Response(&mut resp),
                Phase::Response,
                &context,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .unwrap();

        let Node::Reasoning {
            content,
            summary,
            encrypted,
            extra_body,
            ..
        } = &resp.output[0]
        else {
            panic!("expected reasoning node");
        };
        assert!(encrypted.is_none());
        assert!(!extra_body.contains_key("encrypted_content"));
        assert_eq!(content.as_deref(), Some("plaintext cot"));
        assert_eq!(summary.as_deref(), Some("summary"));
    }

    #[tokio::test]
    async fn clears_encrypted_on_reasoning_delta_stream_event() {
        let transform = StripEncryptedReasoningTransform;
        let cfg = transform.parse_config(json!({})).unwrap();
        let mut state: Box<dyn TransformState> = Box::new(NoState);
        let (_tmp, context) = ctx().await;
        let mut event = UrpStreamEvent::NodeDelta {
            node_index: 0,
            delta: NodeDelta::Reasoning {
                content: Some("partial cot".into()),
                encrypted: Some(json!("mz2.zzzz")),
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
                &context,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .unwrap();
        let UrpStreamEvent::NodeDelta {
            delta: NodeDelta::Reasoning {
                encrypted, content, ..
            },
            ..
        } = event
        else {
            panic!("expected reasoning delta");
        };
        assert!(encrypted.is_none());
        assert_eq!(content.as_deref(), Some("partial cot"));
    }

    #[tokio::test]
    async fn clears_encrypted_content_on_reasoning_envelope_extra_node_start() {
        let transform = StripEncryptedReasoningTransform;
        let cfg = transform.parse_config(json!({})).unwrap();
        let mut state: Box<dyn TransformState> = Box::new(NoState);
        let (_tmp, context) = ctx().await;
        let mut event = UrpStreamEvent::NodeStart {
            node_index: 0,
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            extra_body: [
                ("type".to_string(), json!("reasoning")),
                ("encrypted_content".to_string(), json!("mz2.payload")),
                ("id".to_string(), json!("rs_1")),
            ]
            .into_iter()
            .collect(),
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
            .unwrap();
        let UrpStreamEvent::NodeStart { extra_body, .. } = event else {
            panic!("expected envelope extra start");
        };
        assert!(!extra_body.contains_key("encrypted_content"));
        assert_eq!(extra_body.get("id"), Some(&json!("rs_1")));
        assert_eq!(extra_body.get("type"), Some(&json!("reasoning")));
    }

    #[tokio::test]
    async fn strips_encrypted_in_response_done_output() {
        let transform = StripEncryptedReasoningTransform;
        let cfg = transform.parse_config(json!({})).unwrap();
        let mut state: Box<dyn TransformState> = Box::new(NoState);
        let (_tmp, context) = ctx().await;
        let mut event = UrpStreamEvent::ResponseDone {
            finish_reason: None,
            usage: None,
            output: vec![Node::Reasoning {
                id: Some("rs_1".into()),
                content: None,
                encrypted: Some(json!("mz2.zzz")),
                summary: Some("s".into()),
                source: None,
                extra_body: HashMap::new(),
            }],
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
            .unwrap();
        let UrpStreamEvent::ResponseDone { output, .. } = event else {
            panic!("expected response done");
        };
        let Node::Reasoning {
            encrypted, summary, ..
        } = &output[0]
        else {
            panic!("expected reasoning");
        };
        assert!(encrypted.is_none());
        assert_eq!(summary.as_deref(), Some("s"));
    }

    #[tokio::test]
    async fn ignores_request_phase_data() {
        let transform = StripEncryptedReasoningTransform;
        let cfg = transform.parse_config(json!({})).unwrap();
        let mut state: Box<dyn TransformState> = Box::new(NoState);
        let (_tmp, context) = ctx().await;
        let mut req = crate::urp::UrpRequest {
            model: "m".into(),
            input: vec![Node::Reasoning {
                id: Some("rs_1".into()),
                content: None,
                encrypted: Some(json!("mz2.zzz")),
                summary: None,
                source: None,
                extra_body: HashMap::new(),
            }],
            stream: None,
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
        };
        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Response,
                &context,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .unwrap();
        // Request input must remain untouched: this transform is response-only.
        let Node::Reasoning { encrypted, .. } = &req.input[0] else {
            panic!("expected reasoning");
        };
        assert!(encrypted.is_some());
    }

    #[tokio::test]
    async fn registry_includes_strip_encrypted_reasoning() {
        let registry = crate::transforms::registry();
        assert!(registry.contains_key("strip_encrypted_reasoning"));
        // Touch OrdinaryRole so the import is not dead-coded under cfg gates.
        let _ = OrdinaryRole::Assistant;
    }
}

//! `reasoning_strip_encrypted` response-phase transform.
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

pub struct ReasoningStripEncryptedTransform;

#[async_trait]
impl Transform for ReasoningStripEncryptedTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_strip_encrypted"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Reasoning: strip encrypted payloads"),
            ("zh", "推理：移除加密载荷"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Removes encrypted reasoning payloads from response nodes, stream events, and passthrough state while preserving plaintext reasoning.",
            ),
            (
                "zh",
                "从响应节点、流事件与透传状态中移除加密推理载荷，保留明文推理内容。",
            ),
        ]
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
    factory: || Box::new(ReasoningStripEncryptedTransform),
});

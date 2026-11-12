//! The wire frames of one content block: a complete lifecycle, the deltas of an open block,
//! and the fragments an open block still owes when its node completes (STR-15).

use super::blocks::{
    BlockPayload, messages_provider_block, non_empty, provider_input_json, signature_text,
    wire_signature,
};
use super::wire::Wire;
use crate::error::AppResult;
use crate::urp::{Node, NodeDelta, tool_call_arguments_for_wire};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Item-id prefix of a reasoning node that transports one complete call signature (CTRL-8a).
const CALL_SIGNATURE_ITEM_PREFIX: &str = "rs_gemini_call_";

/// The complete lifecycle of a block that never streamed (DM9a).
pub(super) async fn emit_complete(
    wire: &Wire,
    index: u32,
    payload: &BlockPayload,
) -> AppResult<()> {
    wire.block_start(index, payload.start_block()).await?;
    match payload {
        BlockPayload::Text {
            content, citations, ..
        } => {
            wire.text(index, content).await?;
            for citation in citations {
                wire.citation(index, citation).await?;
            }
        }
        // A redacted block carries its payload in `content_block_start` and has no deltas.
        BlockPayload::Thinking {
            metadata, thinking, ..
        } if !metadata.redacted => {
            if !thinking.is_empty() {
                wire.thinking(index, thinking_delta(None), thinking).await?;
            }
            if let Some(signature) = payload.wire_signature() {
                wire.signature(index, &signature).await?;
            }
        }
        BlockPayload::Thinking { .. } => {}
        BlockPayload::ToolUse { arguments, .. } => wire.partial_json(index, arguments).await?,
        BlockPayload::ProviderItem { deltas, .. } => {
            for delta in deltas {
                wire.provider_delta(index, delta).await?;
            }
        }
    }
    wire.block_stop(index).await
}

/// One `NodeDelta` of the open block at `index`. `extra_body` holds the delta's extras.
pub(super) async fn emit_delta(
    wire: &Wire,
    index: u32,
    payload: &BlockPayload,
    delta: &NodeDelta,
    extra_body: &HashMap<String, Value>,
) -> AppResult<()> {
    match (payload, delta) {
        (
            BlockPayload::Text { .. },
            NodeDelta::Text {
                content, citations, ..
            },
        ) => {
            for citation in citations {
                wire.citation(index, citation).await?;
            }
            wire.text(index, content).await
        }
        (BlockPayload::Text { .. }, NodeDelta::Refusal { content, .. }) => {
            wire.text(index, content).await
        }
        (
            BlockPayload::Thinking {
                metadata, item_id, ..
            },
            NodeDelta::Reasoning {
                metadata: delta_metadata,
                content,
                encrypted,
                summary,
                ..
            },
        ) => {
            if metadata.redacted {
                return Ok(());
            }
            // DM5a: an unmarked summary may duplicate raw content, so only marked summaries
            // stream as thinking text.
            let text = non_empty(content.as_deref()).or_else(|| {
                delta_metadata
                    .summary_as_thinking
                    .then(|| non_empty(summary.as_deref()))
                    .flatten()
            });
            let fragment = Fragment {
                text,
                signature: signature_text(encrypted.as_ref()),
                estimated_tokens: extra_body.get("estimated_tokens"),
            };
            emit_reasoning(wire, index, item_id.as_deref(), fragment).await
        }
        (BlockPayload::ToolUse { .. }, NodeDelta::ToolCallArguments { arguments }) => {
            wire.partial_json(index, arguments).await
        }
        (BlockPayload::ProviderItem { .. }, NodeDelta::ProviderItem { data }) => {
            wire.provider_delta(index, data).await
        }
        _ => Ok(()),
    }
}

/// What a block accumulated while it waited, sent right after its `content_block_start`.
pub(super) async fn emit_accumulated(
    wire: &Wire,
    index: u32,
    payload: &BlockPayload,
) -> AppResult<()> {
    match payload {
        BlockPayload::Text {
            content, citations, ..
        } => {
            wire.text(index, content).await?;
            for citation in citations {
                wire.citation(index, citation).await?;
            }
            Ok(())
        }
        BlockPayload::Thinking {
            metadata,
            thinking,
            signature,
            item_id,
            ..
        } => {
            if metadata.redacted {
                return Ok(());
            }
            let fragment = Fragment {
                text: non_empty(Some(thinking.as_str())),
                signature: signature.clone().filter(|signature| !signature.is_empty()),
                estimated_tokens: None,
            };
            emit_reasoning(wire, index, item_id.as_deref(), fragment).await
        }
        BlockPayload::ToolUse { arguments, .. } => {
            wire.partial_json(index, &tool_call_arguments_for_wire(arguments))
                .await
        }
        BlockPayload::ProviderItem { deltas, .. } => {
            for delta in deltas {
                wire.provider_delta(index, delta).await?;
            }
            Ok(())
        }
    }
}

/// STR-15: the fragments of the complete `node` that the deltas of the open block at `index`
/// did not carry, sent before its `content_block_stop`.
pub(super) async fn emit_suffix(
    wire: &Wire,
    index: u32,
    payload: &BlockPayload,
    node: &Node,
) -> AppResult<()> {
    match (payload, node) {
        (
            BlockPayload::Text {
                content, citations, ..
            },
            Node::Text {
                content: complete,
                citations: complete_citations,
                ..
            },
        ) => {
            for citation in complete_citations.iter().skip(citations.len()) {
                wire.citation(index, citation).await?;
            }
            wire.text(index, suffix(content, complete).unwrap_or_default())
                .await
        }
        (
            BlockPayload::Text { content, .. },
            Node::Refusal {
                content: complete, ..
            },
        ) => {
            wire.text(index, suffix(content, complete).unwrap_or_default())
                .await
        }
        (
            BlockPayload::Thinking {
                metadata,
                thinking,
                signature,
                item_id,
                ..
            },
            Node::Reasoning {
                content,
                summary,
                encrypted,
                ..
            },
        ) => {
            if metadata.redacted {
                return Ok(());
            }
            let complete_text =
                non_empty(content.as_deref()).or_else(|| non_empty(summary.as_deref()));
            let complete_signature = signature_text(encrypted.as_ref());
            let streamed_signature = signature.as_deref().unwrap_or_default();
            let fragment = Fragment {
                text: complete_text.and_then(|complete| suffix(thinking, complete)),
                signature: complete_signature
                    .as_deref()
                    .and_then(|complete| suffix(streamed_signature, complete))
                    .map(str::to_string),
                estimated_tokens: None,
            };
            emit_reasoning(wire, index, item_id.as_deref(), fragment).await
        }
        (
            BlockPayload::ToolUse { arguments, .. },
            Node::ToolCall {
                arguments: complete,
                ..
            },
        ) => {
            wire.partial_json(index, suffix(arguments, complete).unwrap_or_default())
                .await
        }
        (BlockPayload::ProviderItem { body, deltas }, Node::ProviderItem { .. }) => {
            let Some(complete) = messages_provider_block(node) else {
                return Ok(());
            };
            let (Some(streamed), Some(complete)) = (
                provider_input_json(body, deltas),
                provider_input_json(&complete, &[]),
            ) else {
                return Ok(());
            };
            match complete
                .strip_prefix(streamed.as_str())
                .filter(|fragment| !fragment.is_empty())
            {
                Some(fragment) => {
                    let delta = json!({ "type": "input_json_delta", "partial_json": fragment });
                    wire.provider_delta(index, &delta).await
                }
                None => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// One reasoning fragment of an open thinking block.
struct Fragment<'a> {
    text: Option<&'a str>,
    signature: Option<String>,
    /// PM6e: the upstream `estimated_tokens` of the delta, JSON `null` included.
    estimated_tokens: Option<&'a Value>,
}

/// Thinking text, then the signature, as MSG-6 orders them.
async fn emit_reasoning(
    wire: &Wire,
    index: u32,
    item_id: Option<&str>,
    fragment: Fragment<'_>,
) -> AppResult<()> {
    let delta = thinking_delta(fragment.estimated_tokens);
    if let Some(text) = fragment.text {
        wire.thinking(index, delta, text).await?;
    } else if fragment
        .estimated_tokens
        .is_some_and(|value| !value.is_null())
    {
        // PM6e: an empty `thinking_delta` still reports the token estimate.
        wire.delta(index, delta).await?;
    }
    let Some(signature) = fragment.signature else {
        return Ok(());
    };
    // A call-signature transport carries one complete signature, which takes the envelope;
    // other signature deltas may be fragments and pass through unchanged.
    let signature = match item_id.filter(|id| id.starts_with(CALL_SIGNATURE_ITEM_PREFIX)) {
        Some(id) => wire_signature(&signature, Some(id)),
        None => signature,
    };
    wire.signature(index, &signature).await
}

/// A `thinking_delta` template carrying the upstream `estimated_tokens` member (PM6e).
fn thinking_delta(estimated_tokens: Option<&Value>) -> Value {
    let mut delta = Map::from_iter([
        ("type".to_string(), json!("thinking_delta")),
        ("thinking".to_string(), json!("")),
    ]);
    if let Some(estimated_tokens) = estimated_tokens {
        delta.insert("estimated_tokens".to_string(), estimated_tokens.clone());
    }
    Value::Object(delta)
}

/// The part of `complete` after `streamed`, when `complete` strictly extends it.
fn suffix<'a>(streamed: &str, complete: &'a str) -> Option<&'a str> {
    if complete.len() <= streamed.len() {
        return None;
    }
    complete.strip_prefix(streamed)
}

use super::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, Node, NodeDelta, NodeHeader, ProviderProtocol,
    ReasoningMetadata, UrpResponse, UrpStreamEvent, tool_signature,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Prefix for the signature sigil used to smuggle a reasoning item id through an
/// Anthropic `thinking` / `redacted_thinking` block when the downstream client (Claude Code, etc.)
/// is known to strip unknown content-block fields. Format:
///   `mz1.<item_id>.<original_signature>`
/// The sigil is only attached when encoding reasoning toward the **downstream** (response encode).
/// When encoding toward an **upstream** request, a sigil-encoded signature is unwrapped so that the
/// upstream receives only the original opaque payload. See
/// `spec/unified_responses_proxy.spec.md` DM5.2 / PM5b.
const REASONING_SIGNATURE_SIGIL_PREFIX: &str = "mz1.";

pub const REASONING_ENVELOPE_PREFIX: &str = "mz2.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningEnvelope {
    pub v: u8,
    pub provider_type: String,
    pub model: String,
    pub item_id: Option<String>,
    pub payload: Value,
}

/// Wrap `(item_id, signature)` into a sigil string suitable for smuggling through a downstream
/// Anthropic `thinking.signature` or `redacted_thinking.data` field. Returns `None` when either
/// input is empty.
pub fn wrap_reasoning_signature_with_item_id(item_id: &str, signature: &str) -> Option<String> {
    if item_id.is_empty() || signature.is_empty() {
        return None;
    }
    if signature.starts_with(REASONING_SIGNATURE_SIGIL_PREFIX) {
        return Some(signature.to_string());
    }
    Some(format!(
        "{REASONING_SIGNATURE_SIGIL_PREFIX}{item_id}.{signature}"
    ))
}

/// Parse a sigil-encoded signature string into `(item_id, original_signature)`. Returns `None`
/// when the input is not sigil-encoded or is malformed. The item id segment is the substring
/// between the prefix and the first `.`; everything after that `.` is the original signature
/// returned verbatim.
pub fn unwrap_reasoning_signature_sigil(signature: &str) -> Option<(String, String)> {
    let rest = signature.strip_prefix(REASONING_SIGNATURE_SIGIL_PREFIX)?;
    let dot = rest.find('.')?;
    let id = &rest[..dot];
    let original = &rest[dot + 1..];
    if id.is_empty() || original.is_empty() {
        return None;
    }
    Some((id.to_string(), original.to_string()))
}

/// If `signature` is sigil-encoded, return the original signature. Otherwise return it unchanged.
pub fn strip_reasoning_signature_sigil(signature: &str) -> String {
    unwrap_reasoning_signature_sigil(signature)
        .map(|(_, original)| original)
        .unwrap_or_else(|| signature.to_string())
}

pub fn parse_reasoning_envelope(value: &Value) -> Option<ReasoningEnvelope> {
    let raw = value.as_str()?;
    if let Some(encoded) = raw.strip_prefix(REASONING_ENVELOPE_PREFIX) {
        let decoded = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).ok()?;
        let envelope = serde_json::from_slice::<ReasoningEnvelope>(&decoded).ok()?;
        if envelope.v == 2 && !envelope.provider_type.is_empty() && !envelope.model.is_empty() {
            return Some(envelope);
        }
        return None;
    }

    let (item_id, payload) = unwrap_reasoning_signature_sigil(raw)?;
    Some(ReasoningEnvelope {
        v: 1,
        provider_type: String::new(),
        model: String::new(),
        item_id: Some(item_id),
        payload: Value::String(payload),
    })
}

fn reasoning_envelope_matches(
    envelope: &ReasoningEnvelope,
    provider_type: &str,
    model: &str,
) -> bool {
    // PR4c.6: Anthropic drops thinking the current model cannot read and requires an unbroken
    // replay, so Messages-to-Messages replay ignores the model.
    envelope.v == 1
        || (envelope.provider_type == provider_type
            && (provider_type == "messages" || envelope.model == model))
}

fn wrap_reasoning_payload(
    encrypted: &mut Option<Value>,
    item_id: Option<&str>,
    provider_type: &str,
    model: &str,
) {
    let Some(payload) = encrypted.take() else {
        return;
    };
    if !encrypted_value_is_non_empty(&payload) || parse_reasoning_envelope(&payload).is_some() {
        *encrypted = Some(payload);
        return;
    }

    let envelope = ReasoningEnvelope {
        v: 2,
        provider_type: provider_type.to_string(),
        model: model.to_string(),
        item_id: item_id.filter(|id| !id.is_empty()).map(str::to_string),
        payload,
    };
    let Ok(bytes) = serde_json::to_vec(&envelope) else {
        *encrypted = Some(envelope.payload);
        return;
    };
    *encrypted = Some(Value::String(format!(
        "{REASONING_ENVELOPE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    )));
}

fn wrap_reasoning_node_envelope(node: &mut Node, provider_type: &str, model: &str) {
    if let Node::ToolCall {
        call_id, signature, ..
    } = node
    {
        wrap_reasoning_payload(
            signature,
            Some(&tool_signature::signature_item_id(call_id)),
            provider_type,
            model,
        );
        return;
    }
    if let Node::Reasoning {
        id,
        encrypted,
        extra_body,
        ..
    } = node
    {
        extra_body.remove("encrypted_content");
        wrap_reasoning_payload(encrypted, id.as_deref(), provider_type, model);
    }
}

pub fn wrap_reasoning_envelopes_in_response(
    response: &mut UrpResponse,
    provider_type: &str,
    model: &str,
) {
    for node in &mut response.output {
        wrap_reasoning_node_envelope(node, provider_type, model);
    }
}

pub fn wrap_reasoning_envelope_in_stream_event(
    event: &mut UrpStreamEvent,
    provider_type: &str,
    model: &str,
) {
    match event {
        UrpStreamEvent::NodeStart {
            header: NodeHeader::ToolCall {
                call_id, signature, ..
            },
            ..
        } => {
            wrap_reasoning_payload(
                signature,
                Some(&tool_signature::signature_item_id(call_id)),
                provider_type,
                model,
            );
        }
        UrpStreamEvent::NodeDelta {
            delta:
                NodeDelta::Reasoning {
                    encrypted,
                    metadata,
                    ..
                },
            ..
        } => {
            wrap_reasoning_payload(encrypted, metadata.item_id.as_deref(), provider_type, model);
        }
        UrpStreamEvent::NodeDone { node, .. } => {
            wrap_reasoning_node_envelope(node, provider_type, model)
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            for entry in output {
                wrap_reasoning_node_envelope(&mut entry.node, provider_type, model);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Default)]
pub struct ReasoningEnvelopeStreamState {
    pending_fragments: HashMap<u32, PendingReasoningEnvelopeFragments>,
    /// Envelopes assembled from fragments at `NodeDone`, keyed by `node_index`. The terminal
    /// entry of that node wins over `NodeDone` (STR-8e), so it must carry the same envelope.
    assembled: HashMap<u32, Value>,
}

#[derive(Debug, Default)]
struct PendingReasoningEnvelopeFragments {
    values: Vec<Value>,
    item_id: Option<String>,
    source: Option<String>,
    extra_body: HashMap<String, Value>,
}

impl PendingReasoningEnvelopeFragments {
    fn push(
        &mut self,
        value: Value,
        source: Option<&String>,
        metadata: &ReasoningMetadata,
        extra_body: &HashMap<String, Value>,
    ) {
        self.values.push(value);
        if let Some(source) = source.filter(|source| !source.is_empty()) {
            self.source = Some(source.clone());
        }
        if let Some(item_id) = &metadata.item_id {
            self.item_id = Some(item_id.clone());
        }
        self.extra_body.extend(extra_body.clone());
    }

    fn complete_value(&self) -> Option<Value> {
        if self.values.is_empty() {
            return None;
        }
        if self.values.iter().all(Value::is_string) {
            let mut complete = String::new();
            for value in &self.values {
                complete.push_str(value.as_str().expect("checked string fragment"));
            }
            return (!complete.is_empty()).then_some(Value::String(complete));
        }
        (self.values.len() == 1).then(|| self.values[0].clone())
    }
}

fn encrypted_value_is_non_empty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        _ => true,
    }
}

fn stream_delta_uses_encrypted_fragments(
    provider_type: &str,
    extra_body: &HashMap<String, Value>,
) -> bool {
    match provider_type {
        "messages" => true,
        "chat_completion" => {
            extra_body
                .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
                .and_then(Value::as_object)
                .and_then(|detail| detail.get("type"))
                .and_then(Value::as_str)
                != Some("reasoning.encrypted")
        }
        _ => false,
    }
}

impl ReasoningEnvelopeStreamState {
    pub fn wrap_event(
        &mut self,
        mut event: UrpStreamEvent,
        provider_type: &str,
        model: &str,
    ) -> Vec<UrpStreamEvent> {
        if let UrpStreamEvent::NodeDelta {
            node_index,
            delta:
                NodeDelta::Reasoning {
                    metadata,
                    content,
                    encrypted,
                    summary,
                    source,
                },
            usage,
            extra_body,
        } = &mut event
            && stream_delta_uses_encrypted_fragments(provider_type, extra_body)
            && let Some(value) = encrypted.take().filter(encrypted_value_is_non_empty)
        {
            let has_non_encrypted_payload = content
                .as_deref()
                .is_some_and(|content| !content.is_empty())
                || summary
                    .as_deref()
                    .is_some_and(|summary| !summary.is_empty());
            self.pending_fragments.entry(*node_index).or_default().push(
                value,
                source.as_ref(),
                metadata,
                extra_body,
            );
            if !has_non_encrypted_payload && usage.is_none() {
                return Vec::new();
            }
            return vec![event];
        }

        if let UrpStreamEvent::NodeDone {
            node_index, node, ..
        } = &mut event
            && let Some(pending) = self.pending_fragments.remove(node_index)
        {
            let (node_item_id, terminal_encrypted, terminal_source) = match node {
                Node::Reasoning {
                    id,
                    encrypted,
                    source,
                    ..
                } => (id.clone(), encrypted.clone(), source.clone()),
                _ => (None, None, None),
            };
            let complete = terminal_encrypted
                .filter(encrypted_value_is_non_empty)
                .or_else(|| pending.complete_value());
            if let Some(complete) = complete {
                let item_id = node_item_id.or(pending.item_id);
                let mut wrapped = Some(complete);
                wrap_reasoning_payload(&mut wrapped, item_id.as_deref(), provider_type, model);
                if let Some(wrapped) = wrapped {
                    if let Node::Reasoning { encrypted, .. } = node {
                        *encrypted = Some(wrapped.clone());
                    }
                    self.assembled.insert(*node_index, wrapped.clone());
                    return vec![
                        UrpStreamEvent::NodeDelta {
                            node_index: *node_index,
                            delta: NodeDelta::Reasoning {
                                metadata: ReasoningMetadata {
                                    item_id,
                                    ..Default::default()
                                },
                                content: None,
                                encrypted: Some(wrapped),
                                summary: None,
                                source: terminal_source.or(pending.source),
                            },
                            usage: None,
                            extra_body: pending.extra_body,
                        },
                        event,
                    ];
                }
            }
        }

        if let UrpStreamEvent::ResponseDone { output, .. } = &mut event {
            for entry in output.iter_mut() {
                let Some(assembled) = entry.node_index.and_then(|i| self.assembled.get(&i)) else {
                    continue;
                };
                if let Node::Reasoning { encrypted, .. } = &mut entry.node
                    && !encrypted.as_ref().is_some_and(encrypted_value_is_non_empty)
                {
                    *encrypted = Some(assembled.clone());
                }
            }
        }
        if matches!(
            &event,
            UrpStreamEvent::ResponseDone { .. } | UrpStreamEvent::Error { .. }
        ) {
            self.pending_fragments.clear();
            self.assembled.clear();
        }
        wrap_reasoning_envelope_in_stream_event(&mut event, provider_type, model);
        vec![event]
    }
}

pub fn filter_and_unwrap_reasoning_envelopes_for_upstream(
    nodes: &mut Vec<Node>,
    provider_type: &str,
    model: &str,
    enforce_match: bool,
) {
    if provider_type == ProviderProtocol::Gemini.as_str() {
        super::encode::gemini::retain_gemini_signature_provenance(nodes);
    }
    nodes.retain_mut(|node| {
        if let Node::ToolCall { signature, .. } = node {
            if let Some(envelope) = signature.as_ref().and_then(parse_reasoning_envelope) {
                *signature = if enforce_match
                    && !reasoning_envelope_matches(&envelope, provider_type, model)
                {
                    None
                } else {
                    Some(envelope.payload)
                };
            }
            return true;
        }
        let Node::Reasoning {
            encrypted,
            metadata,
            extra_body,
            ..
        } = node
        else {
            return true;
        };
        if let Some(envelope) = encrypted.as_ref().and_then(parse_reasoning_envelope) {
            if enforce_match && !reasoning_envelope_matches(&envelope, provider_type, model) {
                return false;
            }
            if let Some(item_id) = envelope.item_id.filter(|id| !id.is_empty()) {
                metadata.item_id = Some(item_id);
            }
            *encrypted = Some(envelope.payload);
        }
        extra_body.remove("encrypted_content");
        true
    });
}

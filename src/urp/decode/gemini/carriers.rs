//! Canonical response form of decoded Parts (PG5a, PG5d1, PG5d2, PG8). A carrier is a
//! Reasoning node whose id prefix names what its `encrypted` payload holds.

use crate::urp::encode::sanitize_provider_item_wire_body;
use crate::urp::{Node, ProviderProtocol, ReasoningMetadata};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Id prefix of Reasoning nodes that carry a Gemini-produced signature (provenance marker).
pub(crate) const SIGNATURE_ID_PREFIX: &str = "rs_gemini_";
/// Id prefix of a signature carrier bound to an owning text or media Part.
pub(crate) const PART_SIGNATURE_ID_PREFIX: &str = "rs_gemini_part_";
/// Id prefix of a carrier holding a whole server-tool Part (`toolCall` / `toolResponse`).
pub(crate) const ITEM_CARRIER_ID_PREFIX: &str = "rs_gemini_item_";
/// Id prefix of a carrier holding a `CONTINUATION` token.
pub(crate) const CONTINUATION_ID_PREFIX: &str = "rs_gemini_cont_";
/// `ResponseOutcome.incomplete_extra` member that holds the `CONTINUATION` token.
pub(crate) const CONTINUATION_TOKEN_KEY: &str = "continuation_token";

fn carrier_id(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

/// Bytes of Text content in `nodes`: the position of a following Part in the response text.
pub(crate) fn text_bytes(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text { content, .. } => content.len() as u64,
            _ => 0,
        })
        .sum()
}

/// Owner binding for a text signature:
/// `rs_gemini_part_t<byte length>h<xxh3 hex>o<response text offset>r<random>`.
/// It lets the encoder find the owning text, even among equal texts, after a downstream merged
/// or moved Parts.
fn text_carrier_id(text: &str, offset: u64) -> String {
    format!(
        "{PART_SIGNATURE_ID_PREFIX}t{}h{:016x}o{offset}r{}",
        text.len(),
        xxhash_rust::xxh3::xxh3_64(text.as_bytes()),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Parses the `(byte length, xxh3, response text offset)` owner binding of a text signature
/// carrier id.
pub(crate) fn text_carrier_owner(id: &str) -> Option<(usize, u64, u64)> {
    let rest = id
        .strip_prefix(PART_SIGNATURE_ID_PREFIX)?
        .strip_prefix('t')?;
    let (length, rest) = rest.split_once('h')?;
    let (hash, rest) = rest.split_once('o')?;
    let (offset, _) = rest.split_once('r')?;
    Some((
        length.parse().ok()?,
        u64::from_str_radix(hash, 16).ok()?,
        offset.parse().ok()?,
    ))
}

/// Parses the response text offset of a server-tool Part carrier id.
pub(crate) fn item_carrier_offset(id: &str) -> Option<u64> {
    let rest = id.strip_prefix(ITEM_CARRIER_ID_PREFIX)?.strip_prefix('o')?;
    rest.split_once('r')?.0.parse().ok()
}

fn reasoning_carrier(id: String, payload: Value, extra_body: HashMap<String, Value>) -> Node {
    Node::Reasoning {
        // Stream deltas expose only the metadata, so envelopes keep the carrier id through it.
        metadata: ReasoningMetadata {
            item_id: Some(id.clone()),
            ..Default::default()
        },
        id: Some(id),
        content: None,
        encrypted: Some(payload),
        summary: None,
        source: None,
        extra_body,
    }
}

/// The resumable `continuationToken` of a candidate that stopped with `CONTINUATION`.
pub(crate) fn continuation_token(candidate: &Map<String, Value>) -> Option<&str> {
    if candidate.get("finishReason").and_then(Value::as_str) != Some("CONTINUATION") {
        return None;
    }
    candidate
        .get("continuationToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
}

/// Carrier for a `CONTINUATION` token, appended after the partial output.
pub(crate) fn continuation_carrier(token: &str) -> Node {
    reasoning_carrier(
        carrier_id(CONTINUATION_ID_PREFIX),
        json!(token),
        HashMap::new(),
    )
}

/// Encodes a server-tool Part as an opaque carrier payload (unpadded base64url of its JSON).
fn item_carrier(body: &Value, offset: u64) -> Node {
    let wire = sanitize_provider_item_wire_body(body);
    reasoning_carrier(
        carrier_id(&format!("{ITEM_CARRIER_ID_PREFIX}o{offset}r")),
        json!(URL_SAFE_NO_PAD.encode(wire.to_string())),
        HashMap::new(),
    )
}

/// Decodes an item carrier payload back into the original Part.
pub(crate) fn item_carrier_part(payload: &Value) -> Option<Value> {
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_str()?).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
}

/// Renders a Gemini code-execution Part as assistant text so non-Gemini clients can see it.
fn render_code_execution(node: Node) -> Node {
    let Node::ProviderItem {
        origin_protocol: ProviderProtocol::Gemini,
        role,
        item_type,
        body,
        ..
    } = &node
    else {
        return node;
    };
    let content = match item_type.as_str() {
        "executableCode" => {
            let code = &body["executableCode"];
            format!(
                "\n```{}\n{}\n```\n",
                code.get("language")
                    .and_then(Value::as_str)
                    .filter(|language| *language != "LANGUAGE_UNSPECIFIED")
                    .unwrap_or("")
                    .to_ascii_lowercase(),
                code.get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_end_matches('\n')
            )
        }
        "codeExecutionResult" => {
            let result = &body["codeExecutionResult"];
            let outcome = result
                .get("outcome")
                .and_then(Value::as_str)
                .filter(|outcome| *outcome != "OUTCOME_OK")
                .map(|outcome| format!("[{outcome}]\n"))
                .unwrap_or_default();
            format!(
                "\n```output\n{outcome}{}\n```\n",
                result
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim_end_matches('\n')
            )
        }
        _ => return node,
    };
    Node::Text {
        signature: body.get("thoughtSignature").cloned(),
        id: None,
        role: *role,
        content,
        phase: None,
        logprobs: None,
        citations: Vec::new(),
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }
}

/// Canonical response form for one decoded Gemini Part node.
/// Text and media Part signatures move into a following Reasoning carrier with id prefix
/// `rs_gemini_part_` (text carriers also record their owner binding); a signed empty text Part
/// becomes one standalone carrier with id prefix `rs_gemini_`; a signed thought receives an
/// `rs_gemini_` id; server-tool Parts become `rs_gemini_item_` carriers. Function-call
/// signatures stay on the ToolCall.
pub(crate) fn project_response_node(node: Node, text_offset: u64) -> Vec<Node> {
    if let Node::ProviderItem {
        origin_protocol: ProviderProtocol::Gemini,
        item_type,
        body,
        ..
    } = &node
        && matches!(item_type.as_str(), "toolCall" | "toolResponse")
    {
        return vec![item_carrier(body, text_offset)];
    }
    let mut node = render_code_execution(node);
    let (signature, carrier) = match &mut node {
        Node::Text {
            content,
            signature,
            citations,
            logprobs,
            extra_body,
            ..
        } => {
            if content.is_empty()
                && citations.is_empty()
                && logprobs.is_none()
                && let Some(signature) = signature.take()
            {
                return vec![reasoning_carrier(
                    carrier_id(SIGNATURE_ID_PREFIX),
                    signature,
                    std::mem::take(extra_body),
                )];
            }
            let signature = signature.take();
            let carrier = signature
                .as_ref()
                .map(|_| text_carrier_id(content, text_offset))
                .unwrap_or_default();
            (signature, carrier)
        }
        Node::Image { metadata, .. }
        | Node::Audio { metadata, .. }
        | Node::File { metadata, .. } => (
            metadata.signature.take(),
            carrier_id(PART_SIGNATURE_ID_PREFIX),
        ),
        Node::Reasoning {
            id: id @ None,
            encrypted: Some(_),
            ..
        } => {
            *id = Some(carrier_id(SIGNATURE_ID_PREFIX));
            (None, String::new())
        }
        _ => (None, String::new()),
    };
    match signature {
        Some(signature) => vec![node, reasoning_carrier(carrier, signature, HashMap::new())],
        None => vec![node],
    }
}

/// True for a Reasoning node that carries only a signature (no thought text).
pub(crate) fn is_signature_carrier(node: &Node) -> bool {
    matches!(
        node,
        Node::Reasoning {
            content,
            summary,
            encrypted: Some(_),
            ..
        } if content.as_deref().is_none_or(str::is_empty)
            && summary.as_deref().is_none_or(str::is_empty)
    )
}

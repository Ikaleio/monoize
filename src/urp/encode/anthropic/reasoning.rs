use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, Node, REASONING_ENVELOPE_PREFIX,
    strip_reasoning_signature_sigil, wrap_reasoning_signature_with_item_id,
};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;

/// Who receives an encoded thinking block (DM5.2, PM5b, PM5d).
///
/// The real Anthropic API (`Upstream`) receives a clean opaque signature and rejects
/// unsigned thinking. A downstream client may strip unknown fields, so `Downstream` blocks
/// carry the reasoning item id inside the signature sigil (`mz2.…`) for recovery when the
/// client echoes the history back, and keep plaintext thinking even without a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThinkingAudience {
    Upstream,
    Downstream,
}

pub(super) fn plaintext_from_reasoning<'a>(
    content: &'a Option<String>,
    summary: &'a Option<String>,
) -> Option<&'a str> {
    content
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| summary.as_deref().filter(|s| !s.is_empty()))
}

pub(super) fn encoded_signature_value(
    encrypted: &Option<Value>,
    id: &Option<String>,
    audience: ThinkingAudience,
) -> Option<Value> {
    let raw = match encrypted {
        None | Some(Value::Null) => return None,
        Some(Value::String(s)) if s.is_empty() => return None,
        Some(Value::String(s)) => s.clone(),
        Some(other) => return Some(other.clone()),
    };
    let processed = match audience {
        ThinkingAudience::Downstream if raw.starts_with(REASONING_ENVELOPE_PREFIX) => raw,
        ThinkingAudience::Downstream => id
            .as_deref()
            .filter(|s| !s.is_empty())
            .and_then(|item_id| wrap_reasoning_signature_with_item_id(item_id, &raw))
            .unwrap_or(raw),
        ThinkingAudience::Upstream => strip_reasoning_signature_sigil(&raw),
    };
    if processed.is_empty() {
        None
    } else {
        Some(Value::String(processed))
    }
}

/// Yields `nodes` with each adjacent Chat `reasoning.text` + `reasoning.encrypted` pair
/// folded into one signed thinking node, the only Messages form that keeps both.
pub(super) fn with_merged_chat_reasoning(nodes: &[Node]) -> impl Iterator<Item = Cow<'_, Node>> {
    let mut index = 0;
    std::iter::from_fn(move || {
        let node = nodes.get(index)?;
        if let Some(merged) = nodes
            .get(index + 1)
            .and_then(|next| merge_adjacent_chat_reasoning(node, next))
        {
            index += 2;
            return Some(Cow::Owned(merged));
        }
        index += 1;
        Some(Cow::Borrowed(node))
    })
}

fn chat_reasoning_detail_type(extra_body: &HashMap<String, Value>) -> Option<&str> {
    extra_body
        .get(CHAT_REASONING_DETAIL_EXTRA_KEY)
        .and_then(Value::as_object)
        .and_then(|detail| detail.get("type"))
        .and_then(Value::as_str)
}

fn encrypted_reasoning_is_nonempty(encrypted: &Option<Value>) -> bool {
    match encrypted {
        None | Some(Value::Null) => false,
        Some(Value::String(value)) => !value.is_empty(),
        Some(_) => true,
    }
}

fn merge_adjacent_chat_reasoning(plaintext_node: &Node, encrypted_node: &Node) -> Option<Node> {
    let Node::Reasoning {
        metadata: plaintext_metadata,
        id: plaintext_id,
        content: plaintext_content,
        encrypted: plaintext_encrypted,
        summary: plaintext_summary,
        source: plaintext_source,
        extra_body: plaintext_extra,
    } = plaintext_node
    else {
        return None;
    };
    let Node::Reasoning {
        metadata: encrypted_metadata,
        id: encrypted_id,
        content: encrypted_content,
        encrypted,
        summary: encrypted_summary,
        source: encrypted_source,
        extra_body: encrypted_extra,
    } = encrypted_node
    else {
        return None;
    };

    if chat_reasoning_detail_type(plaintext_extra) != Some("reasoning.text")
        || chat_reasoning_detail_type(encrypted_extra) != Some("reasoning.encrypted")
        || plaintext_content.as_deref().is_none_or(str::is_empty)
        || encrypted_reasoning_is_nonempty(plaintext_encrypted)
        || encrypted_content
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        || encrypted_summary
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        || !encrypted_reasoning_is_nonempty(encrypted)
        || plaintext_metadata.redacted
        || encrypted_metadata.redacted
    {
        return None;
    }

    let mut extra_body = plaintext_extra.clone();
    for (key, value) in encrypted_extra {
        extra_body
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    let mut metadata = plaintext_metadata.clone();
    metadata.merge(encrypted_metadata);
    Some(Node::Reasoning {
        metadata,
        id: encrypted_id.clone().or_else(|| plaintext_id.clone()),
        content: plaintext_content.clone(),
        encrypted: encrypted.clone(),
        summary: plaintext_summary.clone(),
        source: plaintext_source
            .clone()
            .or_else(|| encrypted_source.clone()),
        extra_body,
    })
}

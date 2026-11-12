use super::ChatDialect;
use crate::urp::{
    CHAT_REASONING_DETAIL_EXTRA_KEY, CHAT_REASONING_SURFACE_EXTRA_KEY,
    CHAT_REASONING_SURFACE_REASONING_CONTENT, CHAT_REASONING_SURFACE_REASONING_OPAQUE, Node,
};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy)]
pub(super) enum ReasoningTarget {
    /// A downstream Chat response message (DC5).
    Response,
    /// An assistant-history message of an upstream Chat request (PC8a).
    Request { dialect: ChatDialect },
}

/// Plain text of a reasoning node: non-empty content, otherwise non-empty summary.
fn reasoning_plain_text<'a>(content: Option<&'a str>, summary: Option<&'a str>) -> Option<&'a str> {
    content
        .filter(|content| !content.is_empty())
        .or(summary.filter(|summary| !summary.is_empty()))
}

/// Assigns sequential `index` values to synthesized details that carry none (DC5).
fn index_reasoning_details(details: &mut [Map<String, Value>], first: u64) -> u64 {
    let mut next = first;
    for detail in details {
        if let Some(index) = detail.get("index").and_then(Value::as_u64) {
            next = next.max(index + 1);
        } else {
            detail.insert("index".into(), json!(next));
            next += 1;
        }
    }
    next
}

/// Writes the Chat reasoning members of one message from its `Reasoning` nodes, in order.
/// Nodes of other kinds are ignored.
pub(super) fn insert_reasoning_fields(
    message: &mut Map<String, Value>,
    nodes: &[&Node],
    target: ReasoningTarget,
) {
    let mut details = Vec::new();
    let mut reasoning_text = String::new();
    let mut reasoning_content = String::new();
    let mut opaque: Option<Value> = None;
    // Synthesized details take indexes above every native one (DC5).
    let mut next_index = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Reasoning { extra_body, .. } => extra_body
                .get(CHAT_REASONING_DETAIL_EXTRA_KEY)?
                .get("index")?
                .as_u64(),
            _ => None,
        })
        .max()
        .map_or(0, |index| index + 1);

    for node in nodes {
        let Node::Reasoning {
            metadata,
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } = node
        else {
            continue;
        };
        let plain = reasoning_plain_text(content.as_deref(), summary.as_deref());
        if let ReasoningTarget::Request { dialect } = target {
            match dialect {
                ChatDialect::OpenAi => continue,
                ChatDialect::DeepSeek => {
                    reasoning_content.push_str(plain.unwrap_or_default());
                    continue;
                }
                ChatDialect::OpenRouter | ChatDialect::Vercel | ChatDialect::Generic => {}
            }
        }
        // A scalar surface owns only the typed field it carried; other typed fields that a
        // transform set still render as details.
        let mut content = content.as_deref();
        let mut encrypted = encrypted.as_ref();
        match extra_body
            .get(CHAT_REASONING_SURFACE_EXTRA_KEY)
            .and_then(Value::as_str)
        {
            Some(CHAT_REASONING_SURFACE_REASONING_CONTENT) => {
                reasoning_content.push_str(content.take().unwrap_or_default());
            }
            Some(CHAT_REASONING_SURFACE_REASONING_OPAQUE) => {
                if let Some(value) = encrypted.take().filter(|value| !value.is_null()) {
                    opaque = Some(value.clone());
                }
            }
            _ => {}
        }
        let plain = reasoning_plain_text(content, summary.as_deref());
        if metadata.chat_content {
            reasoning_content.push_str(plain.unwrap_or_default());
        }
        let native = extra_body.contains_key(CHAT_REASONING_DETAIL_EXTRA_KEY);
        let mut node_details = crate::urp::reasoning::chat_node_details(
            metadata,
            id.as_deref(),
            content,
            summary.as_deref(),
            encrypted,
            source.as_deref(),
            extra_body,
        );
        if !native {
            next_index = index_reasoning_details(&mut node_details, next_index);
        }
        details.extend(node_details.into_iter().map(Value::Object));
        // RC4e: same-Chat details are authoritative in requests; no alias is synthesized.
        if matches!(target, ReasoningTarget::Response) || !native {
            reasoning_text.push_str(plain.unwrap_or_default());
        }
    }

    if !reasoning_text.is_empty() {
        message.insert("reasoning".to_string(), Value::String(reasoning_text));
    }
    if !reasoning_content.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_content),
        );
    }
    if let Some(opaque) = opaque {
        message.insert("reasoning_opaque".to_string(), opaque);
    }
    if !details.is_empty() {
        message.insert("reasoning_details".to_string(), Value::Array(details));
    }
}

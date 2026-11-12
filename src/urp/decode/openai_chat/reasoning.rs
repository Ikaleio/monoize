use crate::urp::{
    CHAT_REASONING_SURFACE_EXTRA_KEY, CHAT_REASONING_SURFACE_REASONING,
    CHAT_REASONING_SURFACE_REASONING_CONTENT, CHAT_REASONING_SURFACE_REASONING_OPAQUE, Node,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Decodes `reasoning_details`, the scalar `reasoning` / `reasoning_content` views and
/// `reasoning_opaque` of one Chat message into Reasoning nodes, in that order.
pub(super) fn reasoning_nodes(message: &Map<String, Value>) -> Vec<Node> {
    let mut nodes: Vec<Node> = message
        .get("reasoning_details")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(crate::urp::reasoning::chat_reasoning_node_from_detail)
        .filter(|node| matches!(node, Node::Reasoning { .. }))
        .collect();

    // CHAT-4b: scalar `reasoning` is a view of non-empty `reasoning_details`.
    let scalar_surfaces: &[(&str, &str)] = if nodes.is_empty() {
        &[
            ("reasoning", CHAT_REASONING_SURFACE_REASONING),
            (
                "reasoning_content",
                CHAT_REASONING_SURFACE_REASONING_CONTENT,
            ),
        ]
    } else {
        &[(
            "reasoning_content",
            CHAT_REASONING_SURFACE_REASONING_CONTENT,
        )]
    };
    for (key, surface) in scalar_surfaces {
        let Some(content) = message
            .get(*key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let summary_is_alias = *key == "reasoning";
        let duplicate = nodes.iter().any(|node| {
            matches!(node, Node::Reasoning { content: existing, summary, .. }
                if existing.as_deref() == Some(content)
                    || (summary_is_alias && summary.as_deref() == Some(content)))
        });
        if !duplicate {
            nodes.push(surface_node(Some(content.to_string()), None, surface));
        }
    }

    if let Some(opaque) = message
        .get("reasoning_opaque")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let duplicate = nodes.iter().any(|node| {
            matches!(node, Node::Reasoning { encrypted: Some(existing), .. }
                if existing.as_str() == Some(opaque))
        });
        if !duplicate {
            nodes.push(surface_node(
                None,
                Some(Value::String(opaque.to_string())),
                CHAT_REASONING_SURFACE_REASONING_OPAQUE,
            ));
        }
    }
    nodes
}

fn surface_node(content: Option<String>, encrypted: Option<Value>, surface: &str) -> Node {
    Node::Reasoning {
        metadata: Default::default(),
        id: None,
        content,
        encrypted,
        summary: None,
        source: None,
        extra_body: HashMap::from([(
            CHAT_REASONING_SURFACE_EXTRA_KEY.to_string(),
            Value::String(surface.to_string()),
        )]),
    }
}

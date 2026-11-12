use super::{
    Node, NodeHeader, ProviderProtocol, ToolResultContent, UrpResponse, UrpStreamEvent, Usage,
    is_internal_key,
};
use serde_json::Value;
use std::collections::HashMap;

fn retain_internal_metadata(extra_body: &mut HashMap<String, Value>) {
    extra_body.retain(|key, _| is_internal_key(key));
}

fn strip_node_extra_body(node: &mut Node) {
    match node {
        Node::Text {
            extra_body, phase, ..
        } => {
            *phase = None;
            retain_internal_metadata(extra_body);
        }
        Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. } => {
            retain_internal_metadata(extra_body);
        }
        Node::ToolResult {
            content,
            extra_body,
            ..
        } => {
            retain_internal_metadata(extra_body);
            for item in content {
                retain_internal_metadata(item.extra_body_mut());
            }
        }
        Node::NextDownstreamEnvelopeExtra { .. } => {}
    }
}

/// XTRA-5: removes nested passthrough state and envelope control nodes.
pub fn strip_nested_extra_body(nodes: &mut Vec<Node>) {
    nodes.iter_mut().for_each(strip_node_extra_body);
    nodes.retain(|node| !matches!(node, Node::NextDownstreamEnvelopeExtra { .. }));
}

/// XTRA-5a item 5: `None` when `id` already carries the prefix of `target`.
fn native_response_id(id: &str, target: ProviderProtocol) -> Option<String> {
    let prefix = match target {
        ProviderProtocol::Responses => "resp_",
        ProviderProtocol::ChatCompletion => "chatcmpl-",
        ProviderProtocol::Messages => "msg_",
        _ => return None,
    };
    if id.is_empty() || id.starts_with(prefix) {
        return None;
    }
    let tail = ["resp_", "chatcmpl-", "msg_"]
        .iter()
        .find_map(|foreign| id.strip_prefix(foreign))
        .unwrap_or(id);
    Some(format!("{prefix}{tail}"))
}

fn strip_foreign_usage(usage: &mut Option<Usage>) {
    if let Some(usage) = usage {
        retain_internal_metadata(&mut usage.extra_body);
    }
}

/// Foreign top-level members removed by [`strip_foreign_response`]. Billing and
/// request logs read them after the downstream body is encoded (XTRA-5a).
#[derive(Debug, Default)]
pub struct ForeignResponseExtras {
    response: HashMap<String, Value>,
    usage: HashMap<String, Value>,
}

impl ForeignResponseExtras {
    /// Merges the removed members back. A transform-set member keeps its value.
    pub fn restore(self, resp: &mut UrpResponse) {
        for (key, value) in self.response {
            resp.extra_body.entry(key).or_insert(value);
        }
        if let Some(usage) = resp.usage.as_mut() {
            for (key, value) in self.usage {
                usage.extra_body.entry(key).or_insert(value);
            }
        }
    }
}

/// XTRA-5a for a non-stream response.
pub fn strip_foreign_response(
    resp: &mut UrpResponse,
    target: ProviderProtocol,
) -> ForeignResponseExtras {
    strip_nested_extra_body(&mut resp.output);
    if let Some(id) = native_response_id(&resp.id, target) {
        resp.id = id;
    }
    let take_foreign = |extra_body: &mut HashMap<String, Value>| {
        let foreign = std::mem::take(extra_body);
        let (internal, foreign) = foreign
            .into_iter()
            .partition(|(key, _)| is_internal_key(key));
        *extra_body = internal;
        foreign
    };
    ForeignResponseExtras {
        response: take_foreign(&mut resp.extra_body),
        usage: resp
            .usage
            .as_mut()
            .map(|usage| take_foreign(&mut usage.extra_body))
            .unwrap_or_default(),
    }
}

/// XTRA-5a for one stream event. Returns `false` for an envelope control event,
/// which the caller must drop.
#[must_use]
pub fn strip_foreign_stream_event(event: &mut UrpStreamEvent, target: ProviderProtocol) -> bool {
    match event {
        UrpStreamEvent::NodeStart {
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            ..
        }
        | UrpStreamEvent::NodeDone {
            node: Node::NextDownstreamEnvelopeExtra { .. },
            ..
        } => return false,
        UrpStreamEvent::ResponseStart {
            usage,
            id,
            extra_body,
            ..
        } => {
            if let Some(native) = native_response_id(id, target) {
                *id = native;
            }
            strip_foreign_usage(usage);
            retain_internal_metadata(extra_body);
        }
        UrpStreamEvent::NodeStart {
            header, extra_body, ..
        } => {
            if let NodeHeader::Text { phase, .. } = header {
                *phase = None;
            }
            retain_internal_metadata(extra_body);
        }
        UrpStreamEvent::NodeDelta {
            usage, extra_body, ..
        } => {
            strip_foreign_usage(usage);
            retain_internal_metadata(extra_body);
        }
        UrpStreamEvent::NodeDone {
            node,
            usage,
            extra_body,
            ..
        } => {
            strip_node_extra_body(node);
            strip_foreign_usage(usage);
            retain_internal_metadata(extra_body);
        }
        UrpStreamEvent::ResponseDone {
            usage,
            output,
            extra_body,
            ..
        } => {
            for entry in output.iter_mut() {
                strip_node_extra_body(&mut entry.node);
            }
            output.retain(|entry| !matches!(entry.node, Node::NextDownstreamEnvelopeExtra { .. }));
            strip_foreign_usage(usage);
            retain_internal_metadata(extra_body);
        }
        UrpStreamEvent::ProviderControl { .. } | UrpStreamEvent::Error { .. } => {}
    }
    true
}

pub fn retain_provider_items_for_protocol(nodes: &mut Vec<Node>, target: ProviderProtocol) {
    for node in nodes.iter_mut() {
        if let Node::ToolResult { content, .. } = node {
            content.retain(|item| {
                !matches!(
                    item,
                    ToolResultContent::ProviderItem {
                        origin_protocol,
                        ..
                    } if *origin_protocol != target
                )
            });
        }
    }
    nodes.retain(|node| {
        !matches!(
            node,
            Node::ProviderItem {
                origin_protocol,
                ..
            } if *origin_protocol != target
        )
    });
}

pub fn remove_downstream_only_reasoning_for_responses(nodes: &mut Vec<Node>) {
    nodes.retain(|node| {
        !matches!(
            node,
            Node::Reasoning { metadata, .. } if metadata.downstream_only
        )
    });
}

use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

/// AFF-5a mirror of `urp::Node` that borrows every field and replaces the
/// flattened `HashMap` extras with a sorted `BTreeMap`. `HashMap` iteration
/// order is randomized per instance, so serializing `Node` directly would
/// hash identical requests to different affinity keys.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CanonicalAffinityNode<'a> {
    Text {
        signature: &'a Option<Value>,
        citations: &'a [crate::urp::Citation],
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        content: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_breakpoint: &'a Option<urp::CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Image {
        metadata: &'a urp::MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a ImageSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Audio {
        metadata: &'a urp::MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a urp::AudioSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    File {
        metadata: &'a urp::MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a urp::FileSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Refusal {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        content: &'a str,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Reasoning {
        metadata: CanonicalAffinityReasoningMetadata<'a>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: &'a Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: &'a Option<String>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ToolCall {
        namespace: &'a Option<String>,
        signature: &'a Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        tool_type: urp::ToolCallType,
        call_id: &'a str,
        name: &'a str,
        arguments: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_breakpoint: &'a Option<urp::CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        origin_protocol: urp::ProviderProtocol,
        role: urp::OrdinaryRole,
        item_type: &'a str,
        body: &'a Value,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ToolResult {
        signature: &'a Option<Value>,
        namespace: &'a Option<String>,
        name: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        tool_type: urp::ToolCallType,
        call_id: &'a str,
        is_error: bool,
        content: Vec<CanonicalAffinityToolResultContent<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_breakpoint: &'a Option<urp::CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    NextDownstreamEnvelopeExtra {
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
}

#[derive(Serialize)]
struct CanonicalAffinityReasoningMetadata<'a> {
    redacted: bool,
    downstream_only: bool,
    chat_content: bool,
    summary_as_thinking: bool,
    item_id: &'a Option<String>,
    summary_parts: Option<CanonicalAffinityReasoningParts<'a>>,
    content_parts: Option<CanonicalAffinityReasoningParts<'a>>,
}

struct CanonicalAffinityReasoningParts<'a>(&'a [urp::ReasoningTextPart]);

impl Serialize for CanonicalAffinityReasoningParts<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        #[derive(Serialize)]
        struct Entry<'a> {
            byte_length: usize,
            #[serde(flatten)]
            extra_body: BTreeMap<&'a String, &'a Value>,
        }
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for part in self.0 {
            sequence.serialize_element(&Entry {
                byte_length: part.byte_length,
                extra_body: sorted_extra(&part.extra_body),
            })?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CanonicalAffinityToolResultContent<'a> {
    Text {
        text: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_breakpoint: &'a Option<urp::CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Image {
        metadata: &'a urp::MediaMetadata,
        source: &'a ImageSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    File {
        metadata: &'a urp::MediaMetadata,
        source: &'a urp::FileSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ProviderItem {
        origin_protocol: urp::ProviderProtocol,
        item_type: &'a str,
        body: &'a Value,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
}

fn sorted_extra(extra_body: &HashMap<String, Value>) -> BTreeMap<&String, &Value> {
    extra_body.iter().collect()
}

fn canonical_affinity_tool_result_content(
    content: &urp::ToolResultContent,
) -> CanonicalAffinityToolResultContent<'_> {
    match content {
        urp::ToolResultContent::Text {
            text,
            extra_body,
            cache_breakpoint,
        } => CanonicalAffinityToolResultContent::Text {
            text,
            cache_breakpoint,
            extra_body: sorted_extra(extra_body),
        },
        urp::ToolResultContent::Image {
            source,
            metadata,
            extra_body,
        } => CanonicalAffinityToolResultContent::Image {
            metadata,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::ToolResultContent::File {
            source,
            metadata,
            extra_body,
        } => CanonicalAffinityToolResultContent::File {
            metadata,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::ToolResultContent::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
        } => CanonicalAffinityToolResultContent::ProviderItem {
            origin_protocol: *origin_protocol,
            item_type,
            body,
            extra_body: sorted_extra(extra_body),
        },
    }
}

pub(super) fn canonical_affinity_node(node: &urp::Node) -> impl Serialize + '_ {
    match node {
        urp::Node::Text {
            logprobs: _,
            signature,
            citations,
            id,
            role,
            content,
            phase,
            extra_body,
            cache_breakpoint,
        } => CanonicalAffinityNode::Text {
            cache_breakpoint,
            signature,
            citations,
            id,
            role: *role,
            content,
            phase,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Image {
            metadata,
            id,
            role,
            source,
            extra_body,
            ..
        } => canonical_image(metadata, id, role, source, extra_body),
        urp::Node::Audio {
            metadata,
            id,
            role,
            source,
            extra_body,
            ..
        } => canonical_audio(metadata, id, role, source, extra_body),
        urp::Node::File {
            metadata,
            id,
            role,
            source,
            extra_body,
            ..
        } => canonical_file(metadata, id, role, source, extra_body),
        urp::Node::Refusal {
            id,
            content,
            extra_body,
            ..
        } => canonical_refusal(id, content, extra_body),
        urp::Node::Reasoning {
            metadata,
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => CanonicalAffinityNode::Reasoning {
            metadata: canonical_affinity_reasoning_metadata(metadata),
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::ToolCall {
            namespace,
            signature,
            id,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
            cache_breakpoint,
            ..
        } => CanonicalAffinityNode::ToolCall {
            cache_breakpoint,
            namespace,
            signature,
            id,
            tool_type: *tool_type,
            call_id,
            name,
            arguments,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::ProviderItem {
            id,
            origin_protocol: protocol,
            role,
            item_type: kind,
            body,
            extra_body: extra,
        } => canonical_provider_item(id, protocol, role, kind, body, extra),
        urp::Node::ToolResult {
            signature,
            namespace,
            name,
            id,
            tool_type,
            call_id,
            is_error,
            content,
            extra_body,
            cache_breakpoint,
            ..
        } => CanonicalAffinityNode::ToolResult {
            cache_breakpoint,
            signature,
            namespace,
            name,
            id,
            tool_type: *tool_type,
            call_id,
            is_error: *is_error,
            content: canonical_tool_result_contents(content),
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::NextDownstreamEnvelopeExtra { extra_body } => {
            canonical_envelope_extra(extra_body)
        }
    }
}

fn canonical_envelope_extra(extra_body: &HashMap<String, Value>) -> CanonicalAffinityNode<'_> {
    CanonicalAffinityNode::NextDownstreamEnvelopeExtra {
        extra_body: sorted_extra(extra_body),
    }
}

fn canonical_image<'a>(
    metadata: &'a urp::MediaMetadata,
    id: &'a Option<String>,
    role: &urp::OrdinaryRole,
    source: &'a ImageSource,
    extra_body: &'a HashMap<String, Value>,
) -> CanonicalAffinityNode<'a> {
    CanonicalAffinityNode::Image {
        metadata,
        id,
        role: *role,
        source,
        extra_body: sorted_extra(extra_body),
    }
}

fn canonical_audio<'a>(
    metadata: &'a urp::MediaMetadata,
    id: &'a Option<String>,
    role: &urp::OrdinaryRole,
    source: &'a urp::AudioSource,
    extra_body: &'a HashMap<String, Value>,
) -> CanonicalAffinityNode<'a> {
    CanonicalAffinityNode::Audio {
        metadata,
        id,
        role: *role,
        source,
        extra_body: sorted_extra(extra_body),
    }
}

fn canonical_file<'a>(
    metadata: &'a urp::MediaMetadata,
    id: &'a Option<String>,
    role: &urp::OrdinaryRole,
    source: &'a urp::FileSource,
    extra_body: &'a HashMap<String, Value>,
) -> CanonicalAffinityNode<'a> {
    CanonicalAffinityNode::File {
        metadata,
        id,
        role: *role,
        source,
        extra_body: sorted_extra(extra_body),
    }
}

fn canonical_refusal<'a>(
    id: &'a Option<String>,
    content: &'a str,
    extra_body: &'a HashMap<String, Value>,
) -> CanonicalAffinityNode<'a> {
    CanonicalAffinityNode::Refusal {
        id,
        content,
        extra_body: sorted_extra(extra_body),
    }
}

fn canonical_provider_item<'a>(
    id: &'a Option<String>,
    protocol: &urp::ProviderProtocol,
    role: &urp::OrdinaryRole,
    kind: &'a str,
    body: &'a Value,
    extra: &'a HashMap<String, Value>,
) -> CanonicalAffinityNode<'a> {
    CanonicalAffinityNode::ProviderItem {
        id,
        origin_protocol: *protocol,
        role: *role,
        item_type: kind,
        body,
        extra_body: sorted_extra(extra),
    }
}

fn canonical_tool_result_contents(
    content: &[urp::ToolResultContent],
) -> Vec<CanonicalAffinityToolResultContent<'_>> {
    content
        .iter()
        .map(canonical_affinity_tool_result_content)
        .collect()
}

fn canonical_affinity_reasoning_metadata(
    metadata: &urp::ReasoningMetadata,
) -> CanonicalAffinityReasoningMetadata<'_> {
    CanonicalAffinityReasoningMetadata {
        redacted: metadata.redacted,
        downstream_only: metadata.downstream_only,
        chat_content: metadata.chat_content,
        summary_as_thinking: metadata.summary_as_thinking,
        item_id: &metadata.item_id,
        summary_parts: metadata
            .summary_parts
            .as_deref()
            .map(CanonicalAffinityReasoningParts),
        content_parts: metadata
            .content_parts
            .as_deref()
            .map(CanonicalAffinityReasoningParts),
    }
}

#[cfg(test)]
mod canonical_field_regressions {
    use super::*;

    fn fingerprint(node: Value) -> String {
        let request: urp::UrpRequest =
            serde_json::from_value(json!({"model":"m","input":[node]})).unwrap();
        affinity_prefix_hash(&request)
    }

    #[test]
    fn affinity_hash_distinguishes_typed_signatures_namespaces_and_media() {
        for (base, field, changed) in [
            (
                json!({"type":"text","role":"assistant","content":"same"}),
                "signature",
                json!("signature"),
            ),
            (
                json!({"type":"text","role":"assistant","content":"same"}),
                "citations",
                json!([urp::Citation::decode(
                    json!({"uri":"https://example.com"}),
                    urp::ProviderProtocol::Gemini
                )]),
            ),
            (
                json!({"type":"tool_call","call_id":"c","name":"run","arguments":"{}"}),
                "namespace",
                json!("functions"),
            ),
            (
                json!({"type":"tool_call","call_id":"c","name":"run","arguments":"{}"}),
                "signature",
                json!("signature"),
            ),
            (
                json!({"type":"tool_result","call_id":"c","content":[]}),
                "namespace",
                json!("functions"),
            ),
            (
                json!({"type":"tool_result","call_id":"c","content":[]}),
                "name",
                json!("run"),
            ),
            (
                json!({"type":"reasoning","content":"same"}),
                "metadata",
                json!({"redacted":true}),
            ),
        ] {
            let mut other = base.clone();
            other[field] = changed;
            assert_ne!(fingerprint(base), fingerprint(other), "missing {field}");
        }
        for kind in ["image", "audio", "file"] {
            let base = json!({"type":kind,"role":"assistant","source":{"type":"base64","media_type":"image/png","data":"bytes"}});
            let mut changed = base.clone();
            changed["metadata"] = json!({"signature":"new","media_type":"image/png","transcript":"words","expires_at":20});
            assert_ne!(
                fingerprint(base),
                fingerprint(changed),
                "missing {kind} metadata"
            );
        }
    }

    #[test]
    fn affinity_reasoning_part_unknown_fields_have_stable_order() {
        let left: urp::Node = serde_json::from_value(json!({"type":"reasoning","content":"same","metadata":{"content_parts":[{"byte_length":4,"z":3,"a":1,"m":2}]}})).unwrap();
        let right: urp::Node = serde_json::from_value(json!({"type":"reasoning","content":"same","metadata":{"content_parts":[{"m":2,"a":1,"z":3,"byte_length":4}]}})).unwrap();
        assert_eq!(
            serde_json::to_string(&canonical_affinity_node(&left)).unwrap(),
            serde_json::to_string(&canonical_affinity_node(&right)).unwrap()
        );
    }
}

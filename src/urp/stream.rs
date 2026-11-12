use super::{
    AudioSource, Citation, FileSource, FinishReason, ImageSource, MediaMetadata, Node,
    OrdinaryRole, ProviderProtocol, ReasoningMetadata, ResponseOutcome, TokenLogprob, ToolCallType,
    Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// One entry of `ResponseDone.output` (STR-8 through STR-8e).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalNode {
    /// The `node_index` of the earlier `NodeStart` that announced this node. Absent for a
    /// node first seen at the terminal boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_index: Option<u32>,
    #[serde(flatten)]
    pub node: Node,
}

impl TerminalNode {
    pub fn streamed(node_index: u32, node: Node) -> Self {
        Self {
            node_index: Some(node_index),
            node,
        }
    }

    pub fn unannounced(node: Node) -> Self {
        Self {
            node_index: None,
            node,
        }
    }

    /// STR-8d: each entry with the announced index it may claim. An index that `announced`
    /// rejects, or that an earlier entry already claimed, resolves to `None`.
    pub fn resolve<'a>(
        output: &'a [TerminalNode],
        announced: impl Fn(u32) -> bool + 'a,
    ) -> impl Iterator<Item = (Option<u32>, &'a Node)> + 'a {
        let mut claimed = std::collections::HashSet::new();
        output.iter().map(move |entry| {
            let index = entry
                .node_index
                .filter(|index| announced(*index) && claimed.insert(*index));
            (index, &entry.node)
        })
    }

    /// The plain node sequence of a terminal snapshot, for a non-stream `UrpResponse`.
    pub fn into_nodes(output: Vec<TerminalNode>) -> Vec<Node> {
        output.into_iter().map(|entry| entry.node).collect()
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UrpStreamEvent {
    ResponseStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        id: String,
        model: String,
        /// Unix seconds of the native response creation; absent when the upstream sent none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<i64>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NodeStart {
        node_index: u32,
        header: NodeHeader,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NodeDelta {
        node_index: u32,
        delta: NodeDelta,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NodeDone {
        node_index: u32,
        node: Node,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ResponseDone {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<ResponseOutcome>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        output: Vec<TerminalNode>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderControl {
        protocol: String,
        event_name: String,
        data: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Error {
        code: Option<String>,
        message: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeHeader {
    Text {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        citations: Vec<Citation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
    Image {
        #[serde(default)]
        metadata: MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
    },
    Audio {
        #[serde(default)]
        metadata: MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
    },
    File {
        #[serde(default)]
        metadata: MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
    },
    Refusal {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        metadata: ReasoningMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    ToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
        name: String,
    },
    ProviderItem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        origin_protocol: ProviderProtocol,
        role: OrdinaryRole,
        item_type: String,
    },
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
    },
    NextDownstreamEnvelopeExtra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeDelta {
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<TokenLogprob>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        citations: Vec<Citation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        content: String,
    },
    Reasoning {
        #[serde(default)]
        metadata: ReasoningMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    Refusal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<TokenLogprob>>,
        content: String,
    },
    ToolCallArguments {
        arguments: String,
    },
    Image {
        source: ImageSource,
    },
    Audio {
        source: AudioSource,
    },
    File {
        source: FileSource,
    },
    ProviderItem {
        data: Value,
    },
}

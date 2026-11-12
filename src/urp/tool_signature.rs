use super::{
    Node, NodeDelta, NodeHeader, ReasoningMetadata, TerminalNode, UrpResponse, UrpStreamEvent,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

const CALL_SIGNATURE_PREFIX: &str = "rs_gemini_call_";

pub fn signature_item_id(call_id: &str) -> String {
    format!("{CALL_SIGNATURE_PREFIX}{}", URL_SAFE_NO_PAD.encode(call_id))
}

fn bound_call_id(item_id: &str) -> Option<String> {
    String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(item_id.strip_prefix(CALL_SIGNATURE_PREFIX)?)
            .ok()?,
    )
    .ok()
}

fn transport_node(call_id: &str, signature: Value) -> Node {
    Node::Reasoning {
        id: Some(signature_item_id(call_id)),
        metadata: ReasoningMetadata::default(),
        content: None,
        summary: None,
        encrypted: Some(signature),
        source: None,
        extra_body: HashMap::new(),
    }
}

/// Moves recognized wire transport into its correlated canonical call before request transforms.
/// An existing typed signature takes precedence. Unmatched transport has no native replay target.
pub fn restore_request_call_signatures(nodes: &mut Vec<Node>) {
    let mut calls = HashMap::<String, Vec<usize>>::new();
    for (index, node) in nodes.iter().enumerate() {
        if let Node::ToolCall { call_id, .. } = node {
            calls.entry(call_id.clone()).or_default().push(index);
        }
    }
    let mut consumed = Vec::new();
    let mut signatures = Vec::new();
    for (index, node) in nodes.iter_mut().enumerate() {
        let Node::Reasoning {
            id,
            metadata,
            content,
            summary,
            encrypted,
            ..
        } = node
        else {
            continue;
        };
        if content.as_deref().is_some_and(|text| !text.is_empty())
            || summary.as_deref().is_some_and(|text| !text.is_empty())
        {
            continue;
        }
        let envelope_id = encrypted
            .as_ref()
            .and_then(super::parse_reasoning_envelope)
            .and_then(|envelope| envelope.item_id);
        let call_id = id
            .as_deref()
            .and_then(bound_call_id)
            .or_else(|| metadata.item_id.as_deref().and_then(bound_call_id))
            .or_else(|| envelope_id.as_deref().and_then(bound_call_id));
        let Some(call_id) = call_id else {
            continue;
        };
        consumed.push(index);
        if let Some(indices) = calls.get(&call_id).filter(|indices| indices.len() == 1)
            && let Some(signature) = encrypted.take()
        {
            signatures.push((indices[0], signature));
        }
    }
    for (index, payload) in signatures {
        if let Node::ToolCall { signature, .. } = &mut nodes[index]
            && signature.is_none()
        {
            *signature = Some(payload);
        }
    }
    let mut index = 0;
    nodes.retain(|_| {
        let keep = consumed.binary_search(&index).is_err();
        index += 1;
        keep
    });
}

/// Builds an encoder-local wire view without adding another signature to canonical storage.
pub fn project_response(response: &UrpResponse) -> UrpResponse {
    let mut projected = response.clone();
    let mut output = Vec::with_capacity(projected.output.len());
    let mut controls = Vec::new();
    for mut node in std::mem::take(&mut projected.output) {
        if matches!(node, Node::NextDownstreamEnvelopeExtra { .. }) {
            controls.push(node);
            continue;
        }
        if let Some((call_id, signature)) = take_call_signature(&mut node) {
            output.push(transport_node(call_id, signature));
        }
        output.append(&mut controls);
        output.push(node);
    }
    output.append(&mut controls);
    projected.output = output;
    projected
}

fn take_call_signature(node: &mut Node) -> Option<(&str, Value)> {
    let Node::ToolCall {
        call_id, signature, ..
    } = node
    else {
        return None;
    };
    Some((call_id.as_str(), signature.take()?))
}

fn is_control_event(event: &UrpStreamEvent) -> bool {
    matches!(
        event,
        UrpStreamEvent::NodeStart {
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            ..
        } | UrpStreamEvent::NodeDone {
            node: Node::NextDownstreamEnvelopeExtra { .. },
            ..
        }
    )
}

/// Stream state of one source node.
struct NodeMapping {
    /// The projected `node_index`.
    node: u32,
    transport: Option<Transport>,
}

/// The bound Reasoning transport of one streamed call signature.
struct Transport {
    node_index: u32,
    /// Ordinary nodes announced before the transport started. Terminal output keeps the
    /// transport at that stream position (PG5a2).
    preceding_nodes: usize,
    completed: bool,
}

/// One ordinary terminal entry with the control entries that target its envelope.
struct TerminalGroup {
    controls: Vec<TerminalNode>,
    entry: TerminalNode,
    /// The announced source index that `entry` claims (STR-8d).
    source: Option<u32>,
    /// The unannounced transport of a signed entry that owns no streamed transport. It directly
    /// precedes `controls` (PG5a2).
    terminal_only_transport: Option<Node>,
}

/// Where a streamed transport goes in terminal output: right after the terminal entry of the
/// last ordinary node announced before it, else before the first one announced after it.
/// Anchors exclude controls and unannounced nodes, and each insertion lands outside a whole
/// control/owner group, so it never separates a control from its target.
fn transport_position(anchors: &[Option<usize>], preceding_nodes: usize, owner: usize) -> usize {
    anchors[..preceding_nodes]
        .iter()
        .rev()
        .find_map(|anchor| anchor.map(|position| position + 1))
        .or_else(|| anchors[preceding_nodes..].iter().find_map(|anchor| *anchor))
        .unwrap_or(owner)
}

/// Projects canonical stream events at the downstream encoder boundary with dense wire indices.
/// Each call signature leaves its call and travels in a bound Reasoning transport (PG5a1).
#[derive(Default)]
pub struct SignatureProjection {
    defer_transport_start: bool,
    next_index: u32,
    /// Every ordinary and control source `node_index` seen so far.
    mapping: HashMap<u32, NodeMapping>,
    /// Source indices of ordinary nodes in announcement order.
    ordinary: Vec<u32>,
    /// Control events wait for the next ordinary event: a transport that event starts must
    /// precede them so that they keep targeting the envelope of that node.
    controls: VecDeque<UrpStreamEvent>,
    ready: VecDeque<UrpStreamEvent>,
}

impl SignatureProjection {
    /// Keeps sequential Messages blocks available for tool argument deltas: a transport starts
    /// only when its signature is final.
    pub fn for_messages() -> Self {
        Self {
            defer_transport_start: true,
            ..Self::default()
        }
    }

    /// Projects one event. Read the projected events with `next_ready`.
    pub fn push(&mut self, mut event: UrpStreamEvent) {
        if is_control_event(&event) {
            self.controls.push_back(event);
            return;
        }
        match &mut event {
            UrpStreamEvent::NodeStart {
                node_index, header, ..
            } => {
                // A start signature is provisional (CTRL-8a): it may open the transport, but
                // only NodeDone or the terminal snapshot supplies its value.
                let signed = match header {
                    NodeHeader::ToolCall {
                        call_id, signature, ..
                    } => signature
                        .take()
                        .filter(|_| !self.defer_transport_start)
                        .map(|signature| (call_id.as_str(), signature)),
                    _ => None,
                };
                *node_index = self.prepare_node(*node_index, signed, false);
            }
            UrpStreamEvent::NodeDelta { node_index, .. } => {
                *node_index = self.prepare_node(*node_index, None, false);
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                let signed = take_call_signature(node);
                *node_index = self.prepare_node(*node_index, signed, true);
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                self.flush_controls();
                *output = self.project_terminal(std::mem::take(output));
            }
            _ => self.flush_controls(),
        }
        self.ready.push_back(event);
    }

    pub fn next_ready(&mut self) -> Option<UrpStreamEvent> {
        self.ready.pop_front()
    }

    /// Releases control nodes held at end of input.
    pub fn finish(&mut self) {
        self.flush_controls();
    }

    fn allocate(&mut self) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    /// Returns the projected index of `source`. A signature starts the transport of the call
    /// before held controls are released; `complete` also emits the transport value.
    fn prepare_node(&mut self, source: u32, signed: Option<(&str, Value)>, complete: bool) -> u32 {
        let mut transport = self
            .mapping
            .get_mut(&source)
            .and_then(|mapping| mapping.transport.take());
        if let Some((call_id, signature)) = signed {
            let started = transport.get_or_insert_with(|| self.start_transport(call_id));
            if complete {
                self.complete_transport(started, call_id, signature);
            }
        }
        self.flush_controls();
        if let Some(mapping) = self.mapping.get_mut(&source) {
            mapping.transport = transport;
            return mapping.node;
        }
        self.ordinary.push(source);
        let node = self.allocate();
        self.mapping.insert(source, NodeMapping { node, transport });
        node
    }

    fn start_transport(&mut self, call_id: &str) -> Transport {
        let node_index = self.allocate();
        self.ready.push_back(UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::Reasoning {
                id: Some(signature_item_id(call_id)),
                metadata: ReasoningMetadata::default(),
            },
            extra_body: HashMap::new(),
        });
        Transport {
            node_index,
            preceding_nodes: self.ordinary.len(),
            completed: false,
        }
    }

    /// Emits the transport value once, from the first completing signature. A different terminal
    /// value only changes the transport's terminal entry (STR-8e).
    fn complete_transport(&mut self, transport: &mut Transport, call_id: &str, signature: Value) {
        if transport.completed {
            return;
        }
        transport.completed = true;
        self.ready.push_back(UrpStreamEvent::NodeDelta {
            node_index: transport.node_index,
            delta: NodeDelta::Reasoning {
                metadata: ReasoningMetadata {
                    item_id: Some(signature_item_id(call_id)),
                    ..Default::default()
                },
                content: None,
                summary: None,
                source: None,
                encrypted: Some(signature.clone()),
            },
            usage: None,
            extra_body: HashMap::new(),
        });
        self.ready.push_back(UrpStreamEvent::NodeDone {
            node_index: transport.node_index,
            node: transport_node(call_id, signature),
            usage: None,
            extra_body: HashMap::new(),
        });
    }

    fn flush_controls(&mut self) {
        while let Some(mut event) = self.controls.pop_front() {
            if let UrpStreamEvent::NodeStart { node_index, .. }
            | UrpStreamEvent::NodeDone { node_index, .. } = &mut event
            {
                *node_index = self.map_control(*node_index);
            }
            self.ready.push_back(event);
        }
    }

    fn map_control(&mut self, source: u32) -> u32 {
        if let Some(mapping) = self.mapping.get(&source) {
            return mapping.node;
        }
        let node = self.allocate();
        self.mapping.insert(
            source,
            NodeMapping {
                node,
                transport: None,
            },
        );
        node
    }

    /// Renumbers the terminal snapshot (STR-8c), completes the transport of each announced call
    /// whose signature only the snapshot carries, and places every transport. An entry reaches
    /// stream state through its index only (STR-8d).
    fn project_terminal(&mut self, output: Vec<TerminalNode>) -> Vec<TerminalNode> {
        let sources: Vec<Option<u32>> =
            TerminalNode::resolve(&output, |index| self.mapping.contains_key(&index))
                .map(|(source, _)| source)
                .collect();
        let mut groups = Vec::new();
        let mut controls = Vec::new();
        for (mut entry, source) in output.into_iter().zip(sources) {
            entry.node_index = source.map(|source| self.mapping[&source].node);
            if matches!(entry.node, Node::NextDownstreamEnvelopeExtra { .. }) {
                controls.push(entry);
            } else {
                groups.push(TerminalGroup {
                    controls: std::mem::take(&mut controls),
                    entry,
                    source,
                    terminal_only_transport: None,
                });
            }
        }
        let claimed: HashMap<u32, usize> = groups
            .iter()
            .enumerate()
            .filter_map(|(position, group)| Some((group.source?, position)))
            .collect();
        // Terminal positions of the ordinary stream nodes, in announcement order.
        let anchors: Vec<Option<usize>> = self
            .ordinary
            .iter()
            .map(|source| claimed.get(source).copied())
            .collect();
        for (rank, anchor) in anchors.iter().enumerate() {
            let Some(position) = *anchor else {
                continue;
            };
            // The snapshot completes every transport that no NodeDone completed (CTRL-8a).
            if let Node::ToolCall {
                call_id,
                signature: Some(signature),
                ..
            } = &groups[position].entry.node
            {
                let signed = Some((call_id.as_str(), signature.clone()));
                self.prepare_node(self.ordinary[rank], signed, true);
            }
        }
        let mut placed = Vec::new();
        for (position, group) in groups.iter_mut().enumerate() {
            // An entry without a signature drops its transport: typed terminal deletion wins.
            let Some((call_id, signature)) = take_call_signature(&mut group.entry.node) else {
                continue;
            };
            let transport = transport_node(call_id, signature);
            match group
                .source
                .and_then(|source| self.mapping[&source].transport.as_ref())
            {
                Some(streamed) => placed.push((
                    transport_position(&anchors, streamed.preceding_nodes, position),
                    TerminalNode::streamed(streamed.node_index, transport),
                )),
                None => group.terminal_only_transport = Some(transport),
            }
        }
        placed.sort_by_key(|(position, transport)| (*position, transport.node_index));
        let mut placed = placed.into_iter().peekable();
        let mut projected = Vec::new();
        for (position, group) in groups.into_iter().enumerate() {
            while let Some((_, transport)) = placed.next_if(|(at, _)| *at == position) {
                projected.push(transport);
            }
            projected.extend(group.terminal_only_transport.map(TerminalNode::unannounced));
            projected.extend(group.controls);
            projected.push(group.entry);
        }
        projected.extend(placed.map(|(_, transport)| transport));
        projected.extend(controls);
        projected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderType;
    use crate::handlers::DownstreamProtocol;
    use crate::urp::stream_encode::{self, StreamEncodeOptions};
    use crate::urp::{self, FinishReason, OrdinaryRole};
    use axum::response::IntoResponse;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn response() -> UrpResponse {
        urp::decode::gemini::decode_response(&json!({"responseId":"resp-test","modelVersion":"gemini-test",
            "candidates":[{"finishReason":"STOP","content":{"role":"model","parts":[
                {"functionCall":{"id":"native-1","name":"click_at","args":{"x":1,"y":2}},"thoughtSignature":"Y2FsbA=="}]}}]})).unwrap()
    }

    fn signature(node: &Node) -> Option<Value> {
        match node {
            Node::ToolCall { signature, .. } => signature.clone(),
            _ => None,
        }
    }

    /// The NodeStart header of `call`, carrying its signature when `signed`.
    fn call_header(call: &Node, signed: bool) -> NodeHeader {
        let mut header = NodeHeader::from(call);
        if let NodeHeader::ToolCall {
            signature: provisional,
            ..
        } = &mut header
        {
            *provisional = signature(call).filter(|_| signed);
        }
        header
    }

    fn named_call(name: &str) -> Node {
        let mut call = response().output.remove(0);
        if let Node::ToolCall {
            call_id,
            name: tool_name,
            signature,
            ..
        } = &mut call
        {
            *call_id = name.into();
            *tool_name = name.into();
            *signature = Some(json!(format!("signature-{name}")));
        }
        call
    }

    fn unsigned(mut call: Node) -> Node {
        take_call_signature(&mut call);
        call
    }

    fn control_node(probe: &str) -> Node {
        Node::NextDownstreamEnvelopeExtra {
            extra_body: HashMap::from([("probe".into(), json!(probe))]),
        }
    }

    fn control_start(node_index: u32, probe: &str) -> UrpStreamEvent {
        UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::NextDownstreamEnvelopeExtra,
            extra_body: HashMap::from([("probe".into(), json!(probe))]),
        }
    }

    fn start(node_index: u32, header: NodeHeader) -> UrpStreamEvent {
        UrpStreamEvent::NodeStart {
            node_index,
            header,
            extra_body: HashMap::new(),
        }
    }

    fn arguments(node_index: u32, arguments: &str) -> UrpStreamEvent {
        UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::ToolCallArguments {
                arguments: arguments.into(),
            },
            usage: None,
            extra_body: HashMap::new(),
        }
    }

    fn done(node_index: u32, node: Node) -> UrpStreamEvent {
        UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body: HashMap::new(),
        }
    }

    fn response_start() -> UrpStreamEvent {
        UrpStreamEvent::ResponseStart {
            usage: None,
            id: "resp_projection".into(),
            model: "client-model".into(),
            created_at: None,
            extra_body: HashMap::new(),
        }
    }

    fn terminal(output: Vec<TerminalNode>) -> UrpStreamEvent {
        UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            output,
            extra_body: HashMap::new(),
        }
    }

    /// Pushes `events` and returns every event they release.
    fn project(
        projection: &mut SignatureProjection,
        events: impl IntoIterator<Item = UrpStreamEvent>,
    ) -> Vec<UrpStreamEvent> {
        let mut released = Vec::new();
        for event in events {
            projection.push(event);
            released.extend(std::iter::from_fn(|| projection.next_ready()));
        }
        released
    }

    fn terminal_output(events: Vec<UrpStreamEvent>) -> Vec<TerminalNode> {
        match events.into_iter().last() {
            Some(UrpStreamEvent::ResponseDone { output, .. }) => output,
            other => panic!("ResponseDone must come last: {other:?}"),
        }
    }

    fn done_indices(events: &[UrpStreamEvent]) -> Vec<u32> {
        events
            .iter()
            .filter_map(|event| match event {
                UrpStreamEvent::NodeDone { node_index, .. } => Some(*node_index),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn typed_transport_moves_payload_and_typed_signature_wins() {
        for binding in ["id", "metadata", "envelope"] {
            let call = unsigned(response().output.remove(0));
            let mut transport = transport_node("native-1", json!("legacy"));
            let Node::Reasoning {
                id,
                metadata,
                encrypted,
                ..
            } = &mut transport
            else {
                panic!()
            };
            match binding {
                "metadata" => metadata.item_id = id.take(),
                "envelope" => {
                    let envelope = urp::ReasoningEnvelope {
                        v: 2,
                        provider_type: "gemini".into(),
                        model: "gemini-test".into(),
                        item_id: id.take(),
                        payload: json!("legacy"),
                    };
                    *encrypted = Some(json!(format!(
                        "mz2.{}",
                        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).unwrap())
                    )));
                }
                _ => {}
            }
            let mut nodes = vec![transport, call];
            restore_request_call_signatures(&mut nodes);
            assert_eq!(nodes.len(), 1, "{binding}");
            urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                &mut nodes,
                "gemini",
                "gemini-test",
                true,
            );
            assert_eq!(signature(&nodes[0]), Some(json!("legacy")), "{binding}");
        }
        let mut nodes = vec![
            transport_node("native-1", json!("old")),
            response().output.remove(0),
        ];
        restore_request_call_signatures(&mut nodes);
        assert_eq!(signature(&nodes[0]), Some(json!("Y2FsbA==")));
        if let Node::ToolCall { signature, .. } = &mut nodes[0] {
            *signature = None;
        }
        restore_request_call_signatures(&mut nodes);
        assert_eq!(signature(&nodes[0]), None);
    }

    #[test]
    fn provider_model_mismatch_removes_signature_without_removing_call() {
        for (provider, model) in [("messages", "gemini-test"), ("gemini", "different")] {
            let mut original = response();
            urp::wrap_reasoning_envelopes_in_response(&mut original, "gemini", "gemini-test");
            urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                &mut original.output,
                provider,
                model,
                true,
            );
            assert_eq!(original.output.len(), 1);
            assert_eq!(signature(&original.output[0]), None);
            assert_eq!(project_response(&original).output.len(), 1);
        }
    }

    #[test]
    fn stream_projection_uses_dense_indices_for_headers_done_and_terminal_only_calls() {
        let mut response = response();
        let call = response.output.remove(0);
        let renamed = |name: &str| {
            let mut call = call.clone();
            if let Node::ToolCall { call_id, .. } = &mut call {
                *call_id = name.into();
            }
            call
        };
        response.output = vec![
            Node::text(OrdinaryRole::Assistant, "prefix"),
            control_node("call"),
            call.clone(),
            renamed("second"),
            renamed("third"),
        ];
        urp::wrap_reasoning_envelopes_in_response(&mut response, "gemini", "gemini-test");
        let [text, control, first, second, third] = <[Node; 5]>::try_from(response.output).unwrap();
        let events = vec![
            response_start(),
            done(0, text.clone()),
            done(1, control.clone()),
            start(2, call_header(&first, true)),
            done(2, first.clone()),
            done(3, second.clone()),
            terminal(vec![
                TerminalNode::streamed(0, text),
                TerminalNode::streamed(1, control),
                TerminalNode::streamed(2, first),
                TerminalNode::streamed(3, second),
                TerminalNode::unannounced(third),
            ]),
        ];
        let events = project(&mut SignatureProjection::default(), events);
        // CTRL-8a: the transport reserved by the signed start (index 1) finishes after the
        // later-indexed control node (index 2).
        assert_eq!(done_indices(&events), vec![0, 2, 1, 3, 4, 5]);
        let output = terminal_output(events);
        assert_eq!(
            output
                .iter()
                .map(|entry| entry.node_index)
                .collect::<Vec<_>>(),
            vec![
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                None,
                None
            ]
        );
        let mut nodes = TerminalNode::into_nodes(output);
        assert!(nodes.iter().all(|node| signature(node).is_none()));
        restore_request_call_signatures(&mut nodes);
        urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
            &mut nodes,
            "gemini",
            "gemini-test",
            true,
        );
        assert_eq!(
            nodes.iter().filter_map(signature).collect::<Vec<_>>(),
            vec![json!("Y2FsbA=="); 3]
        );
    }

    #[test]
    fn late_signature_precedes_completion_without_delaying_arguments() {
        let call = response().output.remove(0);
        let mut projection = SignatureProjection::default();
        let released = project(&mut projection, [start(0, call_header(&call, false))]);
        assert!(matches!(
            released.as_slice(),
            [UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::ToolCall {
                    signature: None,
                    ..
                },
                ..
            }]
        ));
        let released = project(&mut projection, [arguments(0, "{\"x\":1}")]);
        assert!(matches!(
            released.as_slice(),
            [UrpStreamEvent::NodeDelta { node_index: 0, .. }]
        ));
        let released = project(&mut projection, [done(0, call.clone())]);
        assert_eq!(done_indices(&released), vec![1, 0]);
        let output = terminal_output(project(
            &mut projection,
            [terminal(vec![TerminalNode::streamed(0, call.clone())])],
        ));
        assert_eq!(
            output,
            vec![
                TerminalNode::streamed(0, unsigned(call)),
                TerminalNode::streamed(1, transport_node("native-1", json!("Y2FsbA=="))),
            ]
        );
        let mut nodes = TerminalNode::into_nodes(output);
        restore_request_call_signatures(&mut nodes);
        assert_eq!(nodes.len(), 1);
        assert_eq!(signature(&nodes[0]), Some(json!("Y2FsbA==")));
    }

    #[test]
    fn terminal_projection_preserves_controls_and_content_with_nonpositional_indices() {
        let canonical = vec![
            Node::NextDownstreamEnvelopeExtra {
                extra_body: HashMap::from([("status".into(), json!("completed"))]),
            },
            Node::text(OrdinaryRole::Assistant, "before"),
            Node::ProviderItem {
                id: None,
                role: OrdinaryRole::Assistant,
                origin_protocol: urp::ProviderProtocol::Responses,
                item_type: "vendor_content".into(),
                body: json!({"type":"vendor_content", "value":7}),
                extra_body: HashMap::new(),
            },
            Node::text(OrdinaryRole::Assistant, "after"),
        ];
        for sources in [[1, 0, 2, 3], [11, 4, 9, 2]] {
            let mut events: Vec<_> = sources
                .into_iter()
                .zip(&canonical)
                .map(|(index, node)| done(index, node.clone()))
                .collect();
            events.push(terminal(
                sources
                    .into_iter()
                    .zip(&canonical)
                    .map(|(index, node)| TerminalNode::streamed(index, node.clone()))
                    .collect(),
            ));
            let output = terminal_output(project(&mut SignatureProjection::default(), events));
            let expected: Vec<_> = (0..)
                .zip(&canonical)
                .map(|(index, node)| TerminalNode::streamed(index, node.clone()))
                .collect();
            assert_eq!(output, expected, "{sources:?}");
        }
    }

    #[test]
    fn terminal_projection_does_not_restore_deleted_signature() {
        let call = response().output.remove(0);
        for mode in ["terminal_only", "signed_start", "deleted_at_done"] {
            let mut projection = SignatureProjection::default();
            let streamed = mode != "terminal_only";
            if streamed {
                // The provisional signature reserves transport 0; the call becomes index 1.
                project(&mut projection, [start(0, call_header(&call, true))]);
            }
            if mode == "deleted_at_done" {
                let released = project(&mut projection, [done(0, unsigned(call.clone()))]);
                assert_eq!(done_indices(&released), vec![1]);
            }
            let entry = |node_index| TerminalNode {
                node_index: streamed.then_some(node_index),
                node: unsigned(call.clone()),
            };
            let released = project(&mut projection, [terminal(vec![entry(0)])]);
            assert_eq!(
                released.len(),
                1,
                "{mode}: a deleted signature emits no value"
            );
            assert_eq!(terminal_output(released), vec![entry(1)], "{mode}");
        }
    }

    #[test]
    fn terminal_only_signed_group_does_not_displace_late_stream_signature() {
        for include_control in [false, true] {
            let first = named_call("A");
            let second = named_call("B");
            let mut projection = SignatureProjection::default();
            project(
                &mut projection,
                [start(9, call_header(&first, false)), done(9, first.clone())],
            );
            let mut output = Vec::new();
            let mut expected = vec![TerminalNode::unannounced(transport_node(
                "B",
                json!("signature-B"),
            ))];
            if include_control {
                output.push(TerminalNode::unannounced(control_node("B")));
                expected.push(TerminalNode::unannounced(control_node("B")));
            }
            output.extend([
                TerminalNode::unannounced(second.clone()),
                TerminalNode::streamed(9, first.clone()),
            ]);
            expected.extend([
                TerminalNode::unannounced(unsigned(second)),
                TerminalNode::streamed(0, unsigned(first)),
                TerminalNode::streamed(1, transport_node("A", json!("signature-A"))),
            ]);
            assert_eq!(
                terminal_output(project(&mut projection, [terminal(output)])),
                expected,
                "control={include_control}"
            );
        }
    }

    #[test]
    fn signed_controls_keep_dense_indices_with_start_done_or_delta_only_nodes() {
        let call = named_call("A");
        let control = control_node("call");
        for mode in ["start", "done", "delta"] {
            let mut events = vec![control_start(42, "call"), done(42, control.clone())];
            match mode {
                "start" => events.push(start(7, call_header(&call, true))),
                "delta" => events.push(arguments(7, "{}")),
                _ => {}
            }
            events.push(done(7, call.clone()));
            let mut projection = SignatureProjection::default();
            let mut first_indices = Vec::new();
            for event in project(&mut projection, events) {
                if let UrpStreamEvent::NodeStart { node_index, .. }
                | UrpStreamEvent::NodeDelta { node_index, .. }
                | UrpStreamEvent::NodeDone { node_index, .. } = event
                    && !first_indices.contains(&node_index)
                {
                    first_indices.push(node_index);
                }
            }
            assert_eq!(first_indices, vec![0, 1, 2], "{mode}");
            let output = terminal_output(project(
                &mut projection,
                [terminal(vec![
                    TerminalNode::streamed(42, control.clone()),
                    TerminalNode::streamed(7, call.clone()),
                ])],
            ));
            let transport = transport_node("A", json!("signature-A"));
            let expected = if mode == "delta" {
                vec![
                    TerminalNode::streamed(0, control.clone()),
                    TerminalNode::streamed(1, unsigned(call.clone())),
                    TerminalNode::streamed(2, transport),
                ]
            } else {
                vec![
                    TerminalNode::streamed(0, transport),
                    TerminalNode::streamed(1, control.clone()),
                    TerminalNode::streamed(2, unsigned(call.clone())),
                ]
            };
            assert_eq!(output, expected, "{mode}");
        }
        let mut canonical = response();
        canonical.output = vec![control.clone(), call.clone()];
        let projected = project_response(&canonical);
        assert!(matches!(&projected.output[0], Node::Reasoning { .. }));
        assert_eq!(projected.output[1], control);
        assert!(signature(&canonical.output[1]).is_some());
        let released = project(
            &mut SignatureProjection::default(),
            [
                done(42, control),
                terminal(vec![TerminalNode::unannounced(call)]),
            ],
        );
        assert!(matches!(
            released.first(),
            Some(UrpStreamEvent::NodeDone {
                node: Node::NextDownstreamEnvelopeExtra { .. },
                ..
            })
        ));
        assert!(matches!(
            released.last(),
            Some(UrpStreamEvent::ResponseDone { .. })
        ));
    }

    fn options() -> StreamEncodeOptions {
        StreamEncodeOptions {
            logical_model: "client-model".into(),
            sse_max_frame_length: None,
            mask_sensitive_info: false,
            chat_include_usage: true,
            responses_request_echo: None,
            started_at: std::time::Instant::now(),
            replay_reasoning_duration_secs: None,
        }
    }

    async fn sse_body(
        mut wire_rx: mpsc::Receiver<axum::response::sse::Event>,
    ) -> axum::body::Bytes {
        let mut frames = Vec::new();
        while let Some(frame) = wire_rx.recv().await {
            frames.push(Ok::<_, std::convert::Infallible>(frame));
        }
        let body = axum::response::Sse::new(futures_util::stream::iter(frames))
            .into_response()
            .into_body();
        axum::body::to_bytes(body, usize::MAX).await.unwrap()
    }

    async fn live_wire(
        downstream: DownstreamProtocol,
        events: Vec<UrpStreamEvent>,
    ) -> axum::body::Bytes {
        let (tx, rx) = mpsc::channel(events.len());
        for event in events {
            tx.send(event).await.unwrap();
        }
        drop(tx);
        let (wire_tx, wire_rx) = mpsc::channel(256);
        stream_encode::encode_urp_stream(downstream, rx, wire_tx, options())
            .await
            .unwrap();
        sse_body(wire_rx).await
    }

    async fn replay_wire(
        downstream: DownstreamProtocol,
        response: UrpResponse,
    ) -> axum::body::Bytes {
        let (wire_tx, wire_rx) = mpsc::channel(256);
        stream_encode::replay_urp_response(downstream, response, wire_tx, options())
            .await
            .unwrap();
        sse_body(wire_rx).await
    }

    /// Decodes a downstream SSE body as an upstream stream of the same protocol.
    async fn decode_wire(provider: ProviderType, bytes: axum::body::Bytes) -> UrpResponse {
        let handler = crate::handlers::UrpRequest {
            audio_output_format: None,
            model: "client-model".into(),
            max_multiplier: None,
            server_tool_usage_classes: Vec::new(),
            messages_custom_tool_names: Default::default(),
            affinity_explicit: None,
            affinity_prefix_hash: String::new(),
        };
        let (tx, mut rx) = mpsc::channel(256);
        urp::stream_decode::stream_upstream_to_urp_events(
            &handler,
            None,
            provider,
            reqwest::Response::from(axum::http::Response::new(bytes)),
            tx,
            urp::stream_decode::StreamDecodeContext {
                started_at: None,
                runtime_metrics: None,
                idle_timeout_ms: 1000,
            },
        )
        .await
        .unwrap();
        let mut decoded = None;
        while let Some(event) = rx.recv().await {
            match event {
                UrpStreamEvent::ResponseDone {
                    outcome,
                    finish_reason,
                    usage,
                    output,
                    extra_body,
                } => {
                    decoded = Some(UrpResponse {
                        outcome,
                        id: "resp_projection".into(),
                        model: "client-model".into(),
                        created_at: None,
                        output: TerminalNode::into_nodes(output),
                        finish_reason,
                        usage,
                        extra_body,
                    });
                }
                UrpStreamEvent::Error { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
        decoded.expect("decoded terminal")
    }

    /// The client's next request: `response` as its assistant turn, followed by the result of
    /// call `native-1` when `tool_result`.
    fn client_turn(
        downstream: DownstreamProtocol,
        response: &UrpResponse,
        tool_result: bool,
    ) -> urp::UrpRequest {
        match downstream {
            DownstreamProtocol::AnthropicMessages => {
                let wire = urp::encode::anthropic::encode_response(response, "client-model");
                let mut messages = vec![json!({"role":"assistant","content":wire["content"]})];
                if tool_result {
                    messages.push(json!({"role":"user","content":[
                        {"type":"tool_result","tool_use_id":"native-1","content":"clicked"}]}));
                }
                urp::decode::anthropic::decode_request(
                    &json!({"model":"client-model","max_tokens":100,"messages":messages}),
                )
                .unwrap()
            }
            DownstreamProtocol::Responses => {
                let wire = urp::encode::openai_responses::encode_response(response, "client-model");
                let mut input = wire["output"].as_array().unwrap().clone();
                if tool_result {
                    input.push(json!({"type":"function_call_output","call_id":"native-1","output":"clicked"}));
                }
                urp::decode::openai_responses::decode_request(
                    &json!({"model":"client-model","input":input}),
                )
                .unwrap()
            }
            DownstreamProtocol::ChatCompletions => {
                let wire = urp::encode::openai_chat::encode_response(response, "client-model");
                let mut messages = vec![wire["choices"][0]["message"].clone()];
                if tool_result {
                    messages
                        .push(json!({"role":"tool","tool_call_id":"native-1","content":"clicked"}));
                }
                urp::decode::openai_chat::decode_request(
                    &json!({"model":"client-model","messages":messages}),
                )
                .unwrap()
            }
        }
    }

    /// A live stream of `response = [text, call]` that streams the call arguments.
    fn live_events(response: &UrpResponse, signed_start: bool) -> Vec<UrpStreamEvent> {
        let (text, call) = (&response.output[0], &response.output[1]);
        vec![
            response_start(),
            start(0, NodeHeader::from(text)),
            UrpStreamEvent::NodeDelta {
                node_index: 0,
                delta: NodeDelta::Text {
                    logprobs: None,
                    citations: Vec::new(),
                    signature: None,
                    content: "Action.".into(),
                },
                usage: None,
                extra_body: HashMap::new(),
            },
            done(0, text.clone()),
            start(1, call_header(call, signed_start)),
            arguments(1, "{\"x\":1,"),
            arguments(1, "\"y\":2}"),
            done(1, call.clone()),
            terminal(vec![
                TerminalNode::streamed(0, text.clone()),
                TerminalNode::streamed(1, call.clone()),
            ]),
        ]
    }

    /// PG5a1: one canonical call signature survives a downstream client tool loop back to Gemini,
    /// from a non-stream response, a live stream, and a synthetic stream alike.
    #[tokio::test]
    async fn client_tool_loops_preserve_one_canonical_signature() {
        for (family, downstream, provider) in [
            (
                "messages",
                DownstreamProtocol::AnthropicMessages,
                ProviderType::Messages,
            ),
            (
                "responses",
                DownstreamProtocol::Responses,
                ProviderType::Responses,
            ),
            (
                "chat",
                DownstreamProtocol::ChatCompletions,
                ProviderType::ChatCompletion,
            ),
        ] {
            for mode in ["raw", "nonstream", "live", "live_late", "replay"] {
                let mut original = response();
                original.output.insert(0, Node::assistant_text("Action."));
                if mode != "raw" {
                    urp::wrap_reasoning_envelopes_in_response(
                        &mut original,
                        "gemini",
                        "gemini-test",
                    );
                }
                let wire = match mode {
                    "raw" | "nonstream" => None,
                    "replay" => Some(replay_wire(downstream, original.clone()).await),
                    _ => Some(live_wire(downstream, live_events(&original, mode == "live")).await),
                };
                let delivered = match wire {
                    Some(bytes) => {
                        let text = std::str::from_utf8(&bytes).unwrap();
                        assert!(
                            text.contains("mz2."),
                            "no wire binding {family} {mode}: {text}"
                        );
                        decode_wire(provider, bytes).await
                    }
                    None => original,
                };
                let mut request = client_turn(downstream, &delivered, true);
                assert!(
                    request
                        .input
                        .iter()
                        .all(|node| !matches!(node, Node::Reasoning { .. })),
                    "lingering transport {family} {mode}"
                );
                assert_eq!(
                    request
                        .input
                        .iter()
                        .filter(|node| matches!(node, Node::ToolCall { .. }))
                        .count(),
                    1,
                    "{family} {mode}"
                );
                urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                    &mut request.input,
                    "gemini",
                    "gemini-test",
                    true,
                );
                let native = urp::encode::gemini::encode_request(&request, "gemini-test");
                let part = native["contents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|content| content["parts"].as_array().unwrap())
                    .find(|part| part.get("functionCall").is_some())
                    .unwrap();
                assert_eq!(part["thoughtSignature"], "Y2FsbA==", "{family} {mode}");
                assert_eq!(part["functionCall"]["id"], "native-1");
                assert_eq!(part["functionCall"]["args"], json!({"x":1,"y":2}));
            }
        }
    }

    /// PG5a2: a control keeps its target call, and parallel signatures keep their stream position
    /// in the downstream terminal.
    #[tokio::test]
    async fn wire_projection_keeps_control_target_and_parallel_late_signatures() {
        for (downstream, provider) in [
            (DownstreamProtocol::Responses, ProviderType::Responses),
            (
                DownstreamProtocol::AnthropicMessages,
                ProviderType::Messages,
            ),
        ] {
            for late in [false, true] {
                let first = named_call("A");
                let second = named_call("B");
                let control = control_node("call-A");
                let mut events = vec![
                    response_start(),
                    control_start(99, "call-A"),
                    done(99, control.clone()),
                ];
                for (index, call) in [(5, &first), (2, &second)] {
                    events.push(start(index, call_header(call, !late)));
                    events.push(arguments(index, "{\"x\":1,\"y\":2}"));
                }
                events.extend([
                    done(5, first.clone()),
                    done(2, second.clone()),
                    terminal(vec![
                        TerminalNode::streamed(99, control),
                        TerminalNode::streamed(5, first),
                        TerminalNode::streamed(2, second),
                    ]),
                ]);
                let bytes = live_wire(downstream, events).await;
                let wire = std::str::from_utf8(&bytes).unwrap().to_owned();
                let values: Vec<Value> = wire
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter(|line| *line != "[DONE]")
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                let restored = if matches!(downstream, DownstreamProtocol::Responses) {
                    let starts: Vec<_> = values
                        .iter()
                        .filter(|value| value["type"] == "response.output_item.added")
                        .map(|value| &value["item"])
                        .collect();
                    let call = starts
                        .iter()
                        .find(|item| item["call_id"] == "A")
                        .expect("call A start");
                    assert_eq!(call["probe"], "call-A", "late={late}: {wire}");
                    assert!(
                        starts
                            .iter()
                            .filter(|item| item["type"] == "reasoning")
                            .all(|item| item.get("probe").is_none()),
                        "{wire}"
                    );
                    let completed = values
                        .iter()
                        .find(|value| value["type"] == "response.completed")
                        .expect("Responses terminal");
                    let output = completed["response"]["output"].as_array().unwrap();
                    let expected = if late {
                        ["function_call", "function_call", "reasoning", "reasoning"]
                    } else {
                        ["reasoning", "function_call", "reasoning", "function_call"]
                    };
                    assert_eq!(
                        output
                            .iter()
                            .map(|item| item["type"].as_str().unwrap())
                            .collect::<Vec<_>>(),
                        expected,
                        "{wire}"
                    );
                    urp::decode::openai_responses::decode_request(
                        &json!({"model":"client-model","input":output}),
                    )
                    .unwrap()
                    .input
                } else {
                    assert!(
                        values.iter().any(|value| value["type"] == "message_stop"),
                        "{wire}"
                    );
                    let blocks: Vec<_> = values
                        .iter()
                        .filter(|value| value["type"] == "content_block_start")
                        .filter_map(|value| value["content_block"]["type"].as_str())
                        .collect();
                    assert_eq!(
                        blocks.iter().filter(|kind| **kind == "tool_use").count(),
                        2,
                        "{wire}"
                    );
                    assert_eq!(
                        blocks
                            .iter()
                            .filter(|kind| matches!(**kind, "thinking" | "redacted_thinking"))
                            .count(),
                        2,
                        "{wire}"
                    );
                    let decoded = decode_wire(provider, bytes).await;
                    client_turn(DownstreamProtocol::AnthropicMessages, &decoded, false).input
                };
                let calls: Vec<_> = restored
                    .iter()
                    .filter_map(|node| match node {
                        Node::ToolCall { call_id, .. } => Some((call_id, signature(node))),
                        _ => None,
                    })
                    .collect();
                assert_eq!(calls.len(), 2, "{wire}");
                for (call_id, stored) in calls {
                    assert_eq!(
                        stored,
                        Some(json!(format!("signature-{call_id}"))),
                        "{wire}"
                    );
                }
            }
        }
    }
}

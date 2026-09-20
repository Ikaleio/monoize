use super::{Node, NodeDelta, NodeHeader, ReasoningMetadata, UrpResponse, UrpStreamEvent};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc;

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
        if let Some(indices) = calls.get(&call_id).filter(|indices| indices.len() == 1) {
            if let Some(signature) = encrypted.take() {
                signatures.push((indices[0], signature));
            }
        }
    }
    for (index, payload) in signatures {
        if let Node::ToolCall { signature, .. } = &mut nodes[index] {
            if signature.is_none() {
                *signature = Some(payload);
            }
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
    let mut output = Vec::new();
    let mut controls = Vec::new();
    for mut node in std::mem::take(&mut projected.output) {
        if matches!(node, Node::NextDownstreamEnvelopeExtra { .. }) {
            controls.push(node);
            continue;
        }
        if let Some((call_id, signature)) = take_call_signature(&mut node) {
            output.push(transport_node(&call_id, signature));
        }
        output.append(&mut controls);
        output.push(node);
    }
    output.append(&mut controls);
    projected.output = output;
    projected
}

fn take_call_signature(node: &mut Node) -> Option<(String, Value)> {
    if let Node::ToolCall {
        call_id, signature, ..
    } = node
    {
        signature
            .take()
            .map(|signature| (call_id.clone(), signature))
    } else {
        None
    }
}

struct NodeMapping {
    node: u32,
    transport: Option<u32>,
    header: Option<NodeHeader>,
    completed: Option<Node>,
}

struct SignatureEmission {
    index: u32,
    preceding_nodes: usize,
    completed: bool,
}

/// Projects canonical stream events at the downstream encoder boundary with dense wire indices.
#[derive(Default)]
pub struct SignatureProjection {
    defer_transport_start: bool,
    next_index: u32,
    mapping: HashMap<u32, NodeMapping>,
    ordinary_indices: Vec<u32>,
    signatures: HashMap<String, SignatureEmission>,
    controls: VecDeque<UrpStreamEvent>,
    pending: VecDeque<UrpStreamEvent>,
}

impl SignatureProjection {
    /// Keeps sequential Messages blocks available for tool argument deltas.
    pub fn for_messages() -> Self {
        Self {
            defer_transport_start: true,
            ..Self::default()
        }
    }

    pub async fn recv(
        &mut self,
        rx: &mut mpsc::Receiver<UrpStreamEvent>,
    ) -> Option<UrpStreamEvent> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
            match rx.recv().await {
                Some(event) => self.push(event),
                None => {
                    self.flush_controls();
                    return self.pending.pop_front();
                }
            }
        }
    }

    fn allocate(&mut self) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    fn insert_mapping(&mut self, original: u32, transport: Option<u32>) -> u32 {
        let node = self.allocate();
        self.mapping.insert(
            original,
            NodeMapping {
                node,
                transport,
                header: None,
                completed: None,
            },
        );
        node
    }

    fn flush_controls(&mut self) {
        while let Some(mut event) = self.controls.pop_front() {
            let original = match &event {
                UrpStreamEvent::NodeStart { node_index, .. }
                | UrpStreamEvent::NodeDone { node_index, .. } => *node_index,
                _ => unreachable!(),
            };
            let mapped = self
                .mapping
                .get(&original)
                .map(|mapping| mapping.node)
                .unwrap_or_else(|| self.insert_mapping(original, None));
            match &mut event {
                UrpStreamEvent::NodeStart { node_index, .. }
                | UrpStreamEvent::NodeDone { node_index, .. } => *node_index = mapped,
                _ => unreachable!(),
            }
            self.pending.push_back(event);
        }
    }

    fn prepare_node(
        &mut self,
        original: u32,
        signed: Option<(String, Value)>,
        complete: bool,
    ) -> u32 {
        let existing = self
            .mapping
            .get(&original)
            .map(|mapping| (mapping.node, mapping.transport));
        let transport = if let Some((call_id, signature)) = signed {
            if let Some((_, Some(transport))) = existing {
                if complete {
                    self.complete_transport(transport, &call_id, signature);
                }
                Some(transport)
            } else {
                let index = self.allocate();
                self.start_transport(index, &call_id);
                if complete {
                    self.complete_transport(index, &call_id, signature);
                }
                Some(index)
            }
        } else {
            existing.and_then(|(_, transport)| transport)
        };
        self.flush_controls();
        if let Some((mapped, _)) = existing {
            self.mapping.get_mut(&original).unwrap().transport = transport;
            mapped
        } else {
            self.ordinary_indices.push(original);
            self.insert_mapping(original, transport)
        }
    }

    fn start_transport(&mut self, index: u32, call_id: &str) {
        self.signatures.insert(
            call_id.to_owned(),
            SignatureEmission {
                index,
                preceding_nodes: self.ordinary_indices.len(),
                completed: false,
            },
        );
        let id = signature_item_id(call_id);
        self.pending.push_back(UrpStreamEvent::NodeStart {
            node_index: index,
            header: NodeHeader::Reasoning {
                id: Some(id.clone()),
                metadata: ReasoningMetadata::default(),
            },
            extra_body: HashMap::new(),
        });
    }

    fn complete_transport(&mut self, index: u32, call_id: &str, signature: Value) {
        let emission = self
            .signatures
            .get_mut(call_id)
            .expect("reserved signature transport");
        if emission.completed {
            return;
        }
        emission.completed = true;
        self.pending.push_back(UrpStreamEvent::NodeDelta {
            node_index: index,
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
        self.pending.push_back(UrpStreamEvent::NodeDone {
            node_index: index,
            node: transport_node(call_id, signature),
            usage: None,
            extra_body: HashMap::new(),
        });
    }

    fn terminal_projection(&self, output: Vec<Node>) -> Vec<Node> {
        let mut groups = Vec::<(Vec<Node>, Node, Option<(String, Value)>)>::new();
        let mut controls = Vec::new();
        for mut node in output {
            if matches!(node, Node::NextDownstreamEnvelopeExtra { .. }) {
                controls.push(node);
            } else {
                let signature = take_call_signature(&mut node);
                groups.push((std::mem::take(&mut controls), node, signature));
            }
        }
        let mut matched_groups = std::collections::HashSet::new();
        let mut anchors = Vec::new();
        for original in &self.ordinary_indices {
            let mapping = &self.mapping[original];
            let available = |index: &usize| !matched_groups.contains(index);
            let exact = mapping.completed.as_ref().and_then(|completed| {
                groups
                    .iter()
                    .enumerate()
                    .position(|(index, (_, node, _))| available(&index) && completed == node)
            });
            let matched = exact
                .or_else(|| {
                    groups.iter().enumerate().position(|(index, (_, node, _))| {
                        available(&index)
                            && mapping.completed.as_ref().is_some_and(|completed| {
                                super::nodes_semantically_match(completed, node)
                            })
                    })
                })
                .or_else(|| {
                    groups.iter().enumerate().position(|(index, (_, node, _))| {
                        available(&index)
                            && mapping
                                .header
                                .as_ref()
                                .is_some_and(|header| header_matches_node(header, node))
                    })
                });
            if let Some(index) = matched {
                matched_groups.insert(index);
            }
            anchors.push(matched);
        }
        let mut boundaries: Vec<Vec<(u32, Node)>> =
            (0..=groups.len()).map(|_| Vec::new()).collect();
        for (owner_index, (_, _, signed)) in groups.iter_mut().enumerate() {
            let Some((call_id, signature)) = signed.as_ref() else {
                continue;
            };
            let Some(emission) = self.signatures.get(call_id) else {
                continue;
            };
            // Anchors exclude controls and terminal-only nodes. Each insertion is
            // outside an entire control/owner group, so it cannot retarget a control.
            let boundary = anchors[..emission.preceding_nodes]
                .iter()
                .rev()
                .find_map(|index| index.map(|index| index + 1))
                .or_else(|| {
                    anchors[emission.preceding_nodes..]
                        .iter()
                        .find_map(|index| *index)
                })
                .unwrap_or(owner_index);
            boundaries[boundary].push((emission.index, transport_node(call_id, signature.clone())));
            *signed = None;
        }
        let mut projected = Vec::new();
        for (index, (mut controls, node, signed)) in groups.into_iter().enumerate() {
            boundaries[index].sort_by_key(|(index, _)| *index);
            projected.extend(boundaries[index].drain(..).map(|(_, node)| node));
            if let Some((call_id, signature)) = signed {
                projected.push(transport_node(&call_id, signature));
            }
            projected.append(&mut controls);
            projected.push(node);
        }
        if let Some(last) = boundaries.last_mut() {
            last.sort_by_key(|(index, _)| *index);
            projected.extend(last.drain(..).map(|(_, node)| node));
        }
        projected.append(&mut controls);
        projected
    }

    fn push(&mut self, mut event: UrpStreamEvent) {
        if matches!(
            &event,
            UrpStreamEvent::NodeStart {
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                ..
            } | UrpStreamEvent::NodeDone {
                node: Node::NextDownstreamEnvelopeExtra { .. },
                ..
            }
        ) {
            self.controls.push_back(event);
            return;
        }
        match &mut event {
            UrpStreamEvent::NodeStart {
                node_index, header, ..
            } => {
                let original = *node_index;
                let signed = if let NodeHeader::ToolCall {
                    call_id, signature, ..
                } = header
                {
                    signature
                        .take()
                        .map(|signature| (call_id.clone(), signature))
                } else {
                    None
                };
                *node_index = self.prepare_node(
                    original,
                    if self.defer_transport_start {
                        None
                    } else {
                        signed
                    },
                    false,
                );
                self.mapping.get_mut(&original).unwrap().header = Some(header.clone());
            }
            UrpStreamEvent::NodeDelta { node_index, .. } => {
                *node_index = self.prepare_node(*node_index, None, false);
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                let original = *node_index;
                let signed = take_call_signature(node);
                *node_index = self.prepare_node(original, signed, true);
                self.mapping.get_mut(&original).unwrap().completed = Some(node.clone());
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                self.flush_controls();
                let terminal_signatures: Vec<_> = self
                    .ordinary_indices
                    .iter()
                    .filter_map(|original| {
                        let mapping = &self.mapping[original];
                        if mapping.transport.is_some() {
                            return None;
                        }
                        output.iter().find_map(|node| {
                            let Node::ToolCall {
                                call_id,
                                signature: Some(signature),
                                ..
                            } = node
                            else {
                                return None;
                            };
                            mapping
                                .header
                                .as_ref()
                                .is_some_and(|header| header_matches_node(header, node))
                                .then(|| (*original, call_id.clone(), signature.clone()))
                        })
                    })
                    .collect();
                for (original, call_id, signature) in terminal_signatures {
                    self.prepare_node(original, Some((call_id, signature)), true);
                }
                let mut unfinished: Vec<_> = self
                    .signatures
                    .iter()
                    .filter(|(_, emission)| !emission.completed)
                    .map(|(call_id, emission)| (call_id.clone(), emission.index))
                    .collect();
                unfinished.sort_by_key(|(_, index)| *index);
                for (call_id, index) in unfinished {
                    let signature = output.iter().find_map(|node| match node {
                        Node::ToolCall {
                            call_id: id,
                            signature,
                            ..
                        } if id == &call_id => signature.clone(),
                        _ => None,
                    });
                    if let Some(signature) = signature {
                        self.complete_transport(index, &call_id, signature);
                    }
                }
                *output = self.terminal_projection(std::mem::take(output));
            }
            _ => self.flush_controls(),
        }
        self.pending.push_back(event);
    }
}

fn header_matches_node(header: &NodeHeader, node: &Node) -> bool {
    match (header, node) {
        (NodeHeader::ToolCall { call_id: left, .. }, Node::ToolCall { call_id: right, .. })
        | (NodeHeader::ToolResult { call_id: left, .. }, Node::ToolResult { call_id: right, .. }) => {
            left == right
        }
        (NodeHeader::Text { id: left, .. }, Node::Text { id: right, .. })
        | (NodeHeader::Image { id: left, .. }, Node::Image { id: right, .. })
        | (NodeHeader::Audio { id: left, .. }, Node::Audio { id: right, .. })
        | (NodeHeader::File { id: left, .. }, Node::File { id: right, .. })
        | (NodeHeader::Refusal { id: left, .. }, Node::Refusal { id: right, .. })
        | (NodeHeader::Reasoning { id: left, .. }, Node::Reasoning { id: right, .. })
        | (NodeHeader::ProviderItem { id: left, .. }, Node::ProviderItem { id: right, .. }) => {
            left == right
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urp::{self, FinishReason, OrdinaryRole};
    use serde_json::json;

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

    fn call_header(node: &Node) -> NodeHeader {
        let Node::ToolCall {
            namespace,
            signature,
            id,
            tool_type,
            call_id,
            name,
            ..
        } = node
        else {
            panic!()
        };
        NodeHeader::ToolCall {
            namespace: namespace.clone(),
            signature: signature.clone(),
            id: id.clone(),
            tool_type: *tool_type,
            call_id: call_id.clone(),
            name: name.clone(),
        }
    }

    #[test]
    fn nonstream_client_tool_loops_preserve_one_canonical_signature() {
        for family in ["messages", "responses", "chat"] {
            for wrapped in [false, true] {
                let mut original = response();
                if wrapped {
                    urp::wrap_reasoning_envelopes_in_response(
                        &mut original,
                        "gemini",
                        "gemini-test",
                    );
                }
                let mut request = match family {
                    "messages" => {
                        let wire =
                            urp::encode::anthropic::encode_response(&original, "client-model");
                        urp::decode::anthropic::decode_request(&json!({"model":"client-model","max_tokens":100,
                            "messages":[{"role":"assistant","content":wire["content"]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"native-1","content":"clicked"}]}]})).unwrap()
                    }
                    "responses" => {
                        let wire = urp::encode::openai_responses::encode_response(
                            &original,
                            "client-model",
                        );
                        let mut input = wire["output"].as_array().unwrap().clone();
                        input.push(json!({"type":"function_call_output","call_id":"native-1","output":"clicked"}));
                        urp::decode::openai_responses::decode_request(
                            &json!({"model":"client-model","input":input}),
                        )
                        .unwrap()
                    }
                    _ => {
                        let wire =
                            urp::encode::openai_chat::encode_response(&original, "client-model");
                        urp::decode::openai_chat::decode_request(&json!({"model":"client-model","messages":[wire["choices"][0]["message"],{"role":"tool","tool_call_id":"native-1","content":"clicked"}]})).unwrap()
                    }
                };
                assert_eq!(original.output.len(), 1, "canonical mutation for {family}");
                assert!(
                    request
                        .input
                        .iter()
                        .all(|node| !matches!(node, Node::Reasoning { .. })),
                    "lingering transport for {family}"
                );
                assert_eq!(
                    request
                        .input
                        .iter()
                        .filter(|node| matches!(node, Node::ToolCall { .. }))
                        .count(),
                    1
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
                assert_eq!(
                    part["thoughtSignature"], "Y2FsbA==",
                    "{family} wrapped={wrapped}"
                );
                assert_eq!(part["functionCall"]["id"], "native-1");
            }
        }
    }

    #[test]
    fn typed_transport_moves_payload_and_typed_signature_wins() {
        for binding in ["id", "metadata", "legacy_extra", "envelope"] {
            let mut call = response().output.remove(0);
            let Node::ToolCall {
                signature: call_signature,
                ..
            } = &mut call
            else {
                panic!()
            };
            *call_signature = None;
            let mut transport = transport_node("native-1", json!("legacy"));
            let Node::Reasoning {
                id,
                metadata,
                encrypted,
                extra_body,
                ..
            } = &mut transport
            else {
                panic!()
            };
            match binding {
                "metadata" => {
                    metadata.item_id = id.take();
                }
                "legacy_extra" => {
                    extra_body.insert(
                        "_monoize_reasoning_envelope_item_id".into(),
                        json!(id.take().unwrap()),
                    );
                }
                "envelope" => {
                    let envelope = super::super::ReasoningEnvelope {
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
            if binding == "legacy_extra" {
                assert_eq!(nodes.len(), 2);
                assert_eq!(signature(&nodes[1]), None);
                continue;
            }
            assert_eq!(nodes.len(), 1, "{binding}");
            urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                &mut nodes,
                "gemini",
                "gemini-test",
                true,
            );
            assert_eq!(signature(&nodes[0]), Some(json!("legacy")));
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

    #[tokio::test]
    async fn stream_projection_uses_dense_indices_for_headers_done_and_terminal_only_calls() {
        let mut first = response().output.remove(0);
        let mut second = first.clone();
        let mut third = first.clone();
        if let Node::ToolCall { call_id, .. } = &mut second {
            *call_id = "second".into();
        }
        if let Node::ToolCall { call_id, .. } = &mut third {
            *call_id = "third".into();
        }
        let text = Node::text(OrdinaryRole::Assistant, "prefix");
        let control = Node::NextDownstreamEnvelopeExtra {
            extra_body: HashMap::new(),
        };
        let mut start = UrpStreamEvent::NodeStart {
            node_index: 2,
            header: call_header(&first),
            extra_body: HashMap::new(),
        };
        urp::wrap_reasoning_envelope_in_stream_event(&mut start, "gemini", "gemini-test");
        let mut response = response();
        response.output = vec![
            text.clone(),
            control.clone(),
            first.clone(),
            second.clone(),
            third.clone(),
        ];
        urp::wrap_reasoning_envelopes_in_response(&mut response, "gemini", "gemini-test");
        first = response.output[2].clone();
        second = response.output[3].clone();
        let events = vec![
            UrpStreamEvent::ResponseStart {
                id: "r".into(),
                model: "m".into(),
                usage: None,
                extra_body: HashMap::new(),
            },
            UrpStreamEvent::NodeDone {
                node_index: 0,
                node: text,
                usage: None,
                extra_body: HashMap::new(),
            },
            UrpStreamEvent::NodeDone {
                node_index: 1,
                node: control,
                usage: None,
                extra_body: HashMap::new(),
            },
            start,
            UrpStreamEvent::NodeDone {
                node_index: 2,
                node: first,
                usage: None,
                extra_body: HashMap::new(),
            },
            UrpStreamEvent::NodeDone {
                node_index: 3,
                node: second,
                usage: None,
                extra_body: HashMap::new(),
            },
            UrpStreamEvent::ResponseDone {
                outcome: None,
                output: response.output,
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                extra_body: HashMap::new(),
            },
        ];
        let (tx, mut rx) = mpsc::channel(32);
        for event in events {
            tx.send(event).await.unwrap();
        }
        drop(tx);
        let mut projection = SignatureProjection::default();
        let mut output = Vec::new();
        while let Some(event) = projection.recv(&mut rx).await {
            output.push(event);
        }
        let done_indices: Vec<_> = output
            .iter()
            .filter_map(|event| match event {
                UrpStreamEvent::NodeDone { node_index, .. } => Some(*node_index),
                _ => None,
            })
            .collect();
        assert_eq!(done_indices, vec![0, 1, 2, 3, 4, 5]);
        let UrpStreamEvent::ResponseDone {
            outcome: _,
            output: mut nodes,
            ..
        } = output.pop().unwrap()
        else {
            panic!()
        };
        assert_eq!(nodes.len(), 8);
        assert!(nodes.iter().filter_map(signature).next().is_none());
        restore_request_call_signatures(&mut nodes);
        urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
            &mut nodes,
            "gemini",
            "gemini-test",
            true,
        );
        assert_eq!(nodes.iter().filter_map(signature).count(), 3);
        assert!(
            nodes
                .iter()
                .filter_map(signature)
                .all(|value| value == json!("Y2FsbA=="))
        );
    }

    #[test]
    fn late_signature_precedes_completion_without_delaying_arguments() {
        let call = response().output.remove(0);
        let mut header = call_header(&call);
        if let NodeHeader::ToolCall { signature, .. } = &mut header {
            *signature = None;
        }
        let mut projection = SignatureProjection::default();
        projection.push(UrpStreamEvent::NodeStart {
            node_index: 0,
            header,
            extra_body: HashMap::new(),
        });
        assert!(matches!(
            projection.pending.pop_front(),
            Some(UrpStreamEvent::NodeStart {
                node_index: 0,
                header: NodeHeader::ToolCall {
                    signature: None,
                    ..
                },
                ..
            })
        ));
        projection.push(UrpStreamEvent::NodeDelta {
            node_index: 0,
            delta: NodeDelta::ToolCallArguments {
                arguments: "{\"x\":1}".into(),
            },
            usage: None,
            extra_body: HashMap::new(),
        });
        assert!(matches!(
            projection.pending.pop_front(),
            Some(UrpStreamEvent::NodeDelta { node_index: 0, .. })
        ));
        projection.push(UrpStreamEvent::NodeDone {
            node_index: 0,
            node: call.clone(),
            usage: None,
            extra_body: HashMap::new(),
        });
        let indices: Vec<_> = projection
            .pending
            .drain(..)
            .filter_map(|event| match event {
                UrpStreamEvent::NodeDone { node_index, .. } => Some(node_index),
                _ => None,
            })
            .collect();
        assert_eq!(indices, vec![1, 0]);
        projection.push(UrpStreamEvent::ResponseDone {
            outcome: None,
            output: vec![call],
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            extra_body: HashMap::new(),
        });
        let UrpStreamEvent::ResponseDone {
            outcome: _,
            output: mut nodes,
            ..
        } = projection.pending.pop_front().unwrap()
        else {
            panic!()
        };
        assert_eq!(nodes.len(), 2);
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
        for indices in [[1, 0, 2, 3], [11, 4, 9, 2]] {
            let mut projection = SignatureProjection::default();
            for (index, node) in indices.into_iter().zip(&canonical) {
                projection.push(UrpStreamEvent::NodeDone {
                    node_index: index,
                    node: node.clone(),
                    usage: None,
                    extra_body: HashMap::new(),
                });
            }
            projection.pending.clear();
            projection.push(UrpStreamEvent::ResponseDone {
                outcome: None,
                output: canonical.clone(),
                finish_reason: Some(FinishReason::Stop),
                usage: None,
                extra_body: HashMap::new(),
            });
            let UrpStreamEvent::ResponseDone {
                outcome: _, output, ..
            } = projection.pending.pop_front().unwrap()
            else {
                panic!()
            };
            assert_eq!(
                serde_json::to_value(output).unwrap(),
                serde_json::to_value(&canonical).unwrap()
            );
        }
    }

    #[test]
    fn terminal_projection_does_not_restore_deleted_signature() {
        for signed_start in [false, true] {
            let mut call = response().output.remove(0);
            let mut projection = SignatureProjection::default();
            if signed_start {
                projection.push(UrpStreamEvent::NodeStart {
                    node_index: 0,
                    header: call_header(&call),
                    extra_body: HashMap::new(),
                });
                projection.pending.clear();
            }
            if let Node::ToolCall { signature, .. } = &mut call {
                *signature = None;
            }
            projection.push(UrpStreamEvent::ResponseDone {
                outcome: None,
                output: vec![call],
                finish_reason: Some(FinishReason::ToolCalls),
                usage: None,
                extra_body: HashMap::new(),
            });
            assert_eq!(projection.pending.len(), 1);
            let UrpStreamEvent::ResponseDone {
                outcome: _,
                output: mut nodes,
                ..
            } = projection.pending.pop_front().unwrap()
            else {
                panic!()
            };
            assert_eq!(nodes.len(), 1);
            assert!(matches!(
                &nodes[0],
                Node::ToolCall {
                    signature: None,
                    ..
                }
            ));
            restore_request_call_signatures(&mut nodes);
            assert_eq!(signature(&nodes[0]), None);
        }
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

    fn done(node_index: u32, node: Node) -> UrpStreamEvent {
        UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body: HashMap::new(),
        }
    }

    fn terminal(output: Vec<Node>) -> UrpStreamEvent {
        UrpStreamEvent::ResponseDone {
            outcome: None,
            output,
            finish_reason: Some(FinishReason::ToolCalls),
            usage: None,
            extra_body: HashMap::new(),
        }
    }

    #[test]
    fn terminal_only_signed_group_does_not_displace_late_stream_signature() {
        for include_control in [false, true] {
            let first = named_call("A");
            let second = named_call("B");
            let mut header = call_header(&first);
            if let NodeHeader::ToolCall { signature, .. } = &mut header {
                *signature = None;
            }
            let mut projection = SignatureProjection::default();
            projection.push(UrpStreamEvent::NodeStart {
                node_index: 9,
                header,
                extra_body: HashMap::new(),
            });
            projection.push(done(9, first.clone()));
            projection.pending.clear();
            let control = Node::NextDownstreamEnvelopeExtra {
                extra_body: HashMap::from([("probe".into(), json!("B"))]),
            };
            let mut canonical = Vec::new();
            if include_control {
                canonical.push(control.clone());
            }
            canonical.extend([second.clone(), first.clone()]);
            projection.push(terminal(canonical));
            let UrpStreamEvent::ResponseDone {
                outcome: _, output, ..
            } = projection.pending.pop_front().unwrap()
            else {
                panic!()
            };
            let mut expected = vec![transport_node("B", json!("signature-B"))];
            if include_control {
                expected.push(control);
            }
            let mut first = first;
            let mut second = second;
            take_call_signature(&mut first);
            take_call_signature(&mut second);
            expected.extend([second, first, transport_node("A", json!("signature-A"))]);
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn signed_controls_keep_dense_indices_with_start_done_or_delta_only_nodes() {
        let call = named_call("A");
        let control = Node::NextDownstreamEnvelopeExtra {
            extra_body: HashMap::from([("probe".into(), json!("call"))]),
        };
        for mode in ["start", "done", "delta"] {
            let mut projection = SignatureProjection::default();
            projection.push(UrpStreamEvent::NodeStart {
                node_index: 42,
                header: NodeHeader::NextDownstreamEnvelopeExtra,
                extra_body: HashMap::from([("probe".into(), json!("call"))]),
            });
            projection.push(done(42, control.clone()));
            match mode {
                "start" => projection.push(UrpStreamEvent::NodeStart {
                    node_index: 7,
                    header: call_header(&call),
                    extra_body: HashMap::new(),
                }),
                "delta" => projection.push(UrpStreamEvent::NodeDelta {
                    node_index: 7,
                    delta: NodeDelta::ToolCallArguments {
                        arguments: "{}".into(),
                    },
                    usage: None,
                    extra_body: HashMap::new(),
                }),
                _ => {}
            }
            projection.push(done(7, call.clone()));
            let mut first_indices = Vec::new();
            for event in projection.pending.drain(..) {
                let index = match event {
                    UrpStreamEvent::NodeStart { node_index, .. }
                    | UrpStreamEvent::NodeDelta { node_index, .. }
                    | UrpStreamEvent::NodeDone { node_index, .. } => node_index,
                    _ => continue,
                };
                if !first_indices.contains(&index) {
                    first_indices.push(index);
                }
            }
            assert_eq!(first_indices, vec![0, 1, 2], "{mode}");
            projection.push(terminal(vec![control.clone(), call.clone()]));
            let UrpStreamEvent::ResponseDone {
                outcome: _, output, ..
            } = projection.pending.pop_front().unwrap()
            else {
                panic!()
            };
            let mut unsigned = call.clone();
            take_call_signature(&mut unsigned);
            let signature = transport_node("A", json!("signature-A"));
            let expected = if mode == "delta" {
                vec![control.clone(), unsigned, signature]
            } else {
                vec![signature, control.clone(), unsigned]
            };
            assert_eq!(output, expected, "{mode}");
        }
        let mut canonical = response();
        canonical.output = vec![control.clone(), call.clone()];
        let projected = project_response(&canonical);
        assert!(matches!(&projected.output[0], Node::Reasoning { .. }));
        assert_eq!(projected.output[1], control);
        assert!(signature(&canonical.output[1]).is_some());
        let mut projection = SignatureProjection::default();
        projection.push(done(42, control));
        projection.push(terminal(vec![call]));
        assert!(matches!(
            projection.pending.front(),
            Some(UrpStreamEvent::NodeDone {
                node: Node::NextDownstreamEnvelopeExtra { .. },
                ..
            })
        ));
        assert!(matches!(
            projection.pending.back(),
            Some(UrpStreamEvent::ResponseDone { .. })
        ));
    }

    #[tokio::test]
    async fn wire_projection_keeps_control_target_and_parallel_late_signatures() {
        use crate::handlers::DownstreamProtocol;
        use axum::response::IntoResponse;
        for downstream in [
            DownstreamProtocol::Responses,
            DownstreamProtocol::AnthropicMessages,
        ] {
            for late in [false, true] {
                let first = named_call("A");
                let second = named_call("B");
                let control = Node::NextDownstreamEnvelopeExtra {
                    extra_body: HashMap::from([("probe".into(), json!("call-A"))]),
                };
                let mut events = vec![
                    UrpStreamEvent::ResponseStart {
                        id: "resp_projection".into(),
                        model: "client-model".into(),
                        usage: None,
                        extra_body: HashMap::new(),
                    },
                    UrpStreamEvent::NodeStart {
                        node_index: 99,
                        header: NodeHeader::NextDownstreamEnvelopeExtra,
                        extra_body: HashMap::from([("probe".into(), json!("call-A"))]),
                    },
                    done(99, control.clone()),
                ];
                for (index, call) in [(5, &first), (2, &second)] {
                    let mut header = call_header(call);
                    if late {
                        if let NodeHeader::ToolCall { signature, .. } = &mut header {
                            *signature = None;
                        }
                    }
                    events.push(UrpStreamEvent::NodeStart {
                        node_index: index,
                        header,
                        extra_body: HashMap::new(),
                    });
                    events.push(UrpStreamEvent::NodeDelta {
                        node_index: index,
                        delta: NodeDelta::ToolCallArguments {
                            arguments: "{\"x\":1,\"y\":2}".into(),
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    });
                }
                events.extend([
                    done(5, first.clone()),
                    done(2, second.clone()),
                    terminal(vec![control, first, second]),
                ]);
                let (tx, rx) = mpsc::channel(64);
                for event in events {
                    tx.send(event).await.unwrap();
                }
                drop(tx);
                let (wire_tx, mut wire_rx) = mpsc::channel(256);
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    urp::stream_encode::encode_urp_stream(
                        downstream,
                        rx,
                        wire_tx,
                        "client-model",
                        std::time::Instant::now(),
                        None,
                        false,
                    ),
                )
                .await
                .expect("projection stalled")
                .unwrap();
                let mut frames = Vec::new();
                while let Some(event) = wire_rx.recv().await {
                    frames.push(Ok::<_, std::convert::Infallible>(event));
                }
                let body = axum::response::Sse::new(futures_util::stream::iter(frames))
                    .into_response()
                    .into_body();
                let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let wire = std::str::from_utf8(&bytes).unwrap();
                let values: Vec<Value> = wire
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter(|line| *line != "[DONE]")
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                let mut restored = if matches!(downstream, DownstreamProtocol::Responses) {
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
                    let terminal = values
                        .iter()
                        .find(|value| value["type"] == "response.completed")
                        .expect("Responses terminal");
                    let output = terminal["response"]["output"].as_array().unwrap();
                    let expected = if late {
                        vec!["function_call", "function_call", "reasoning", "reasoning"]
                    } else {
                        vec!["reasoning", "function_call", "reasoning", "function_call"]
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
                        .map(|value| value["content_block"].clone())
                        .collect();
                    assert_eq!(
                        blocks
                            .iter()
                            .filter(|block| block["type"] == "tool_use")
                            .count(),
                        2,
                        "{wire}"
                    );
                    assert_eq!(
                        blocks
                            .iter()
                            .filter(|block| matches!(
                                block["type"].as_str(),
                                Some("thinking" | "redacted_thinking")
                            ))
                            .count(),
                        2,
                        "{wire}"
                    );
                    let (decoded_tx, mut decoded_rx) = mpsc::channel(256);
                    let handler = crate::handlers::UrpRequest {
                        audio_output_format: None,
                        model: "client-model".into(),
                        max_multiplier: None,
                        server_tool_usage_classes: Vec::new(),
                        messages_custom_tool_names: Default::default(),
                        affinity_explicit: None,
                        affinity_prefix_hash: String::new(),
                    };
                    urp::stream_decode::stream_upstream_to_urp_events(
                        &handler,
                        None,
                        crate::config::ProviderType::Messages,
                        reqwest::Response::from(axum::http::Response::new(bytes.clone())),
                        decoded_tx,
                        None,
                        None,
                        1000,
                    )
                    .await
                    .unwrap();
                    let mut output = None;
                    while let Some(event) = decoded_rx.recv().await {
                        match event {
                            UrpStreamEvent::ResponseDone {
                                outcome: _,
                                output: nodes,
                                ..
                            } => output = Some(nodes),
                            UrpStreamEvent::Error { message, .. } => panic!("{message}: {wire}"),
                            _ => {}
                        }
                    }
                    let response = UrpResponse {
                        outcome: None,
                        id: "resp_projection".into(),
                        model: "client-model".into(),
                        created_at: None,
                        output: output.expect("Messages decoded terminal"),
                        finish_reason: Some(FinishReason::ToolCalls),
                        usage: None,
                        extra_body: HashMap::new(),
                    };
                    let history =
                        urp::encode::anthropic::encode_response(&response, "client-model");
                    urp::decode::anthropic::decode_request(&json!({"model":"client-model","max_tokens":100,"messages":[{"role":"assistant","content":history["content"]}]})).unwrap().input
                };
                restore_request_call_signatures(&mut restored);
                let calls: Vec<_> = restored
                    .iter()
                    .filter_map(|node| {
                        if let Node::ToolCall {
                            call_id, signature, ..
                        } = node
                        {
                            Some((call_id, signature))
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(calls.len(), 2);
                for (call_id, signature) in calls {
                    assert_eq!(
                        signature.as_ref(),
                        Some(&json!(format!("signature-{call_id}"))),
                        "{wire}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn live_and_synthetic_client_tool_loops_preserve_signature() {
        use crate::config::ProviderType;
        use crate::handlers::DownstreamProtocol;
        use axum::response::IntoResponse;
        use futures_util::StreamExt;
        for (family, provider, downstream) in [
            (
                "messages",
                ProviderType::Messages,
                DownstreamProtocol::AnthropicMessages,
            ),
            (
                "responses",
                ProviderType::Responses,
                DownstreamProtocol::Responses,
            ),
            (
                "chat",
                ProviderType::ChatCompletion,
                DownstreamProtocol::ChatCompletions,
            ),
        ] {
            for (synthetic, signature_at_start) in [(false, true), (false, false), (true, true)] {
                let mut original = response();
                original.output.insert(0, Node::assistant_text("Action."));
                urp::wrap_reasoning_envelopes_in_response(&mut original, "gemini", "gemini-test");
                let (wire_tx, wire_rx) = mpsc::channel(256);
                if synthetic {
                    urp::stream_encode::emit_synthetic_stream_from_urp_response(
                        downstream,
                        "client-model",
                        &original,
                        None,
                        None,
                        wire_tx,
                    )
                    .await
                    .unwrap();
                } else {
                    let call = original.output[1].clone();
                    let mut header = call_header(&call);
                    if !signature_at_start {
                        if let NodeHeader::ToolCall { signature, .. } = &mut header {
                            *signature = None;
                        }
                    }
                    let events = vec![
                        UrpStreamEvent::ResponseStart {
                            id: original.id.clone(),
                            model: "client-model".into(),
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeStart {
                            node_index: 0,
                            header: NodeHeader::Text {
                                signature: None,
                                citations: Vec::new(),
                                id: None,
                                role: OrdinaryRole::Assistant,
                                phase: None,
                            },
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeDelta {
                            node_index: 0,
                            delta: NodeDelta::Text {
                                logprobs: None,
                                signature: None,
                                citations: Vec::new(),
                                content: "Action.".into(),
                            },
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeDone {
                            node_index: 0,
                            node: original.output[0].clone(),
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeStart {
                            node_index: 1,
                            header,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeDelta {
                            node_index: 1,
                            delta: NodeDelta::ToolCallArguments {
                                arguments: "{\"x\":1,".into(),
                            },
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeDelta {
                            node_index: 1,
                            delta: NodeDelta::ToolCallArguments {
                                arguments: "\"y\":2}".into(),
                            },
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::NodeDone {
                            node_index: 1,
                            node: call,
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                        UrpStreamEvent::ResponseDone {
                            outcome: None,
                            output: original.output.clone(),
                            finish_reason: Some(FinishReason::ToolCalls),
                            usage: None,
                            extra_body: HashMap::new(),
                        },
                    ];
                    let (tx, rx) = mpsc::channel(32);
                    for event in events {
                        tx.send(event).await.unwrap();
                    }
                    drop(tx);
                    urp::stream_encode::encode_urp_stream(
                        downstream,
                        rx,
                        wire_tx,
                        "client-model",
                        std::time::Instant::now(),
                        None,
                        false,
                    )
                    .await
                    .unwrap();
                }
                let native = axum::response::sse::Sse::new(
                    tokio_stream::wrappers::ReceiverStream::new(wire_rx)
                        .map(Ok::<_, std::convert::Infallible>),
                )
                .into_response();
                let bytes = axum::body::to_bytes(native.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let text = std::str::from_utf8(&bytes).unwrap();
                assert!(
                    text.contains("mz2."),
                    "no wire binding {family} synthetic={synthetic}: {text}"
                );
                let (tx, mut rx) = mpsc::channel(256);
                let handler = crate::handlers::UrpRequest {
                    audio_output_format: None,
                    model: "client-model".into(),
                    max_multiplier: None,
                    server_tool_usage_classes: Vec::new(),
                    messages_custom_tool_names: Default::default(),
                    affinity_explicit: None,
                    affinity_prefix_hash: String::new(),
                };
                urp::stream_decode::stream_upstream_to_urp_events(
                    &handler,
                    None,
                    provider,
                    reqwest::Response::from(axum::http::Response::new(bytes.clone())),
                    tx,
                    None,
                    None,
                    1000,
                )
                .await
                .unwrap();
                let mut decoded = None;
                while let Some(event) = rx.recv().await {
                    if let UrpStreamEvent::ResponseDone {
                        outcome: _,
                        output,
                        finish_reason,
                        usage,
                        extra_body,
                    } = event
                    {
                        decoded = Some(UrpResponse {
                            outcome: None,
                            id: "r".into(),
                            model: "client-model".into(),
                            created_at: None,
                            output,
                            finish_reason,
                            usage,
                            extra_body,
                        });
                    }
                }
                let decoded = decoded.expect("native terminal");
                let mut request = match family {
                    "messages" => {
                        let wire =
                            urp::encode::anthropic::encode_response(&decoded, "client-model");
                        urp::decode::anthropic::decode_request(&json!({"model":"client-model","max_tokens":100,"messages":[{"role":"assistant","content":wire["content"]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"native-1","content":"clicked"}]}]})).unwrap()
                    }
                    "responses" => {
                        let wire = urp::encode::openai_responses::encode_response(
                            &decoded,
                            "client-model",
                        );
                        let mut input = wire["output"].as_array().unwrap().clone();
                        input.push(json!({"type":"function_call_output","call_id":"native-1","output":"clicked"}));
                        urp::decode::openai_responses::decode_request(
                            &json!({"model":"client-model","input":input}),
                        )
                        .unwrap()
                    }
                    _ => {
                        let wire =
                            urp::encode::openai_chat::encode_response(&decoded, "client-model");
                        urp::decode::openai_chat::decode_request(&json!({"model":"client-model","messages":[wire["choices"][0]["message"],{"role":"tool","tool_call_id":"native-1","content":"clicked"}]})).unwrap()
                    }
                };
                urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
                    &mut request.input,
                    "gemini",
                    "gemini-test",
                    true,
                );
                let calls: Vec<_> = request.input.iter().filter_map(signature).collect();
                assert_eq!(
                    calls,
                    vec![json!("Y2FsbA==")],
                    "{family} synthetic={synthetic}: {text}"
                );
                assert!(
                    request
                        .input
                        .iter()
                        .all(|node| !matches!(node, Node::Reasoning { .. })),
                    "lingering transport {family} synthetic={synthetic}"
                );
                let native = urp::encode::gemini::encode_request(&request, "gemini-test");
                let call = native["contents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|content| content["parts"].as_array().unwrap())
                    .find(|part| part.get("functionCall").is_some())
                    .unwrap();
                assert_eq!(call["thoughtSignature"], "Y2FsbA==");
                assert_eq!(call["functionCall"]["args"], json!({"x":1,"y":2}));
            }
        }
    }
}

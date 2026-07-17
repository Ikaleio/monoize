#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponsesOutputZone {
    Message,
    ImageGenerationCall,
    Reasoning,
    FunctionCall,
    ProviderItem,
}

#[derive(Clone, Debug)]
struct ActiveResponsesOutputItem {
    zone: ResponsesOutputZone,
    output_index: usize,
    item_id: String,
    item: Value,
    next_content_index: u64,
    envelope_extra: HashMap<String, Value>,
}

#[derive(Clone, Debug)]
struct StreamedNodeState {
    output_index: usize,
    zone: ResponsesOutputZone,
    content_index: Option<u32>,
    item_id: String,
    phase: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    reasoning_summary_part_added_sent: bool,
    message_start_emitted: bool,
    output_item_start_emitted: bool,
    output_item_start: Option<Value>,
    header: Option<urp::NodeHeader>,
    node_extra_body: HashMap<String, Value>,
    completed_item: Option<Value>,
    is_shared_message_output: bool,
    reasoning_started_at: Option<Instant>,
}

fn terminal_output_node_matches_state(node: &urp::Node, state: &StreamedNodeState) -> bool {
    let header_family_matches = matches!(
        (state.header.as_ref(), node),
        (Some(urp::NodeHeader::Text { .. }), urp::Node::Text { .. })
            | (Some(urp::NodeHeader::Image { .. }), urp::Node::Image { .. })
            | (Some(urp::NodeHeader::Audio { .. }), urp::Node::Audio { .. })
            | (Some(urp::NodeHeader::File { .. }), urp::Node::File { .. })
            | (
                Some(urp::NodeHeader::Refusal { .. }),
                urp::Node::Refusal { .. }
            )
            | (
                Some(urp::NodeHeader::ProviderItem { .. }),
                urp::Node::ProviderItem { .. }
            )
            | (
                Some(urp::NodeHeader::Reasoning { .. }),
                urp::Node::Reasoning { .. }
            )
            | (
                Some(urp::NodeHeader::ToolCall { .. }),
                urp::Node::ToolCall { .. }
            )
            | (
                Some(urp::NodeHeader::ToolResult { .. }),
                urp::Node::ToolResult { .. }
            )
    );

    match node {
        urp::Node::Text { id, .. }
        | urp::Node::Audio { id, .. }
        | urp::Node::File { id, .. }
        | urp::Node::Refusal { id, .. }
        => {
            state.zone == ResponsesOutputZone::Message
                && ((!state.item_id.is_empty() && id.as_deref() == Some(state.item_id.as_str()))
                    || (header_family_matches && id.is_none())
                    || (state.item_id.is_empty() && header_family_matches))
        }
        urp::Node::Image { id, extra_body, .. } => {
            let expected_zone = if extra_body
                .contains_key(urp::RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY)
            {
                ResponsesOutputZone::ImageGenerationCall
            } else {
                ResponsesOutputZone::Message
            };
            state.zone == expected_zone
                && ((!state.item_id.is_empty() && id.as_deref() == Some(state.item_id.as_str()))
                    || (header_family_matches && id.is_none())
                    || (state.item_id.is_empty() && header_family_matches))
        }
        urp::Node::ProviderItem { id, .. } => {
            state.zone == ResponsesOutputZone::ProviderItem
                && ((!state.item_id.is_empty() && id.as_deref() == Some(state.item_id.as_str()))
                    || (header_family_matches && id.is_none())
                    || (state.item_id.is_empty() && header_family_matches))
        }
        urp::Node::Reasoning { id, .. } => {
            state.zone == ResponsesOutputZone::Reasoning
                && ((!state.item_id.is_empty() && id.as_deref() == Some(state.item_id.as_str()))
                    || (header_family_matches && id.is_none())
                    || (state.item_id.is_empty() && header_family_matches))
        }
        urp::Node::ToolCall { id, call_id, .. } => {
            state.zone == ResponsesOutputZone::FunctionCall
                && (state.call_id.as_deref() == Some(call_id.as_str())
                    || (!state.item_id.is_empty() && id.as_deref() == Some(state.item_id.as_str()))
                    || (header_family_matches && id.is_none())
                    || ((state.call_id.is_none() && state.item_id.is_empty())
                        && header_family_matches))
        }
        urp::Node::ToolResult { id, call_id, .. } => {
            state.zone == ResponsesOutputZone::FunctionCall
                && (state.call_id.as_deref() == Some(call_id.as_str())
                    || (!state.item_id.is_empty() && id.as_deref() == Some(state.item_id.as_str()))
                    || (header_family_matches && id.is_none())
                    || ((state.call_id.is_none() && state.item_id.is_empty())
                        && header_family_matches))
        }
        urp::Node::NextDownstreamEnvelopeExtra { .. } => false,
    }
}

fn find_terminal_output_node_for_state(
    output: &[urp::Node],
    preferred_index: usize,
    state: &StreamedNodeState,
    used_positions: &HashSet<usize>,
) -> Option<(usize, urp::Node)> {
    if let Some(candidate) = output.get(preferred_index)
        && !used_positions.contains(&preferred_index)
        && terminal_output_node_matches_state(candidate, state)
    {
        return Some((preferred_index, candidate.clone()));
    }

    output
        .iter()
        .enumerate()
        .find(|(index, node)| {
            !used_positions.contains(index) && terminal_output_node_matches_state(node, state)
        })
        .map(|(index, node)| (index, node.clone()))
}

fn synthesize_terminal_node_from_state(state: &StreamedNodeState) -> Option<urp::Node> {
    let header = state.header.as_ref()?;
    let completed_item = state.completed_item.as_ref();

    match header {
        urp::NodeHeader::Reasoning { id } => {
            let content = completed_item
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let encrypted = completed_item
                .and_then(|item| item.get("encrypted_content").cloned())
                .filter(|value| match value {
                    Value::Null => false,
                    Value::String(value) => !value.is_empty(),
                    Value::Array(value) => !value.is_empty(),
                    Value::Object(value) => !value.is_empty(),
                    _ => true,
                });
            let summary = completed_item
                .and_then(|item| item.get("summary"))
                .and_then(Value::as_array)
                .and_then(|summary| {
                    summary
                        .iter()
                        .find_map(|entry| entry.get("text").and_then(Value::as_str))
                })
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            if content.is_none() && encrypted.is_none() && summary.is_none() {
                return None;
            }
            Some(urp::Node::Reasoning {
                id: id
                    .clone()
                    .or_else(|| (!state.item_id.is_empty()).then(|| state.item_id.clone())),
                content,
                encrypted,
                summary,
                source: completed_item
                    .and_then(|item| item.get("source"))
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                extra_body: state.node_extra_body.clone(),
            })
        }
        urp::NodeHeader::ToolCall { id, call_id, name } => Some(urp::Node::ToolCall {
            id: id
                .clone()
                .or_else(|| (!state.item_id.is_empty()).then(|| state.item_id.clone())),
            call_id: state.call_id.clone().unwrap_or_else(|| call_id.clone()),
            name: state.name.clone().unwrap_or_else(|| name.clone()),
            arguments: completed_item
                .and_then(|item| item.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            extra_body: state.node_extra_body.clone(),
        }),
        urp::NodeHeader::ProviderItem {
            id,
            origin_protocol,
            item_type,
            role,
        } => {
            let body = completed_item.cloned().unwrap_or_else(|| {
                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String(item_type.clone()));
                Value::Object(obj)
            });
            Some(urp::Node::ProviderItem {
                id: id
                    .clone()
                    .or_else(|| (!state.item_id.is_empty()).then(|| state.item_id.clone())),
                origin_protocol: *origin_protocol,
                role: *role,
                item_type: item_type.clone(),
                body,
                extra_body: state.node_extra_body.clone(),
            })
        }
        _ => None,
    }
}

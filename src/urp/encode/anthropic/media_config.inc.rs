pub(crate) fn messages_media_error_body(message: &str) -> Value {
    json!({"type":"error", "error":{"type":"api_error", "message":message}})
}

pub(crate) fn input_only_media_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "image" | "document" | "file" | "audio" | "input_image" | "input_file" | "input_audio"
    )
}

pub(crate) fn validate_response_node(node: &Node) -> Result<(), String> {
    let kind = match node {
        Node::Image { .. } => Some("image"),
        Node::File { .. } => Some("document"),
        Node::Audio { .. } => Some("audio"),
        Node::ToolResult { .. } => Some("tool_result"),
        Node::ProviderItem {
            origin_protocol: ProviderProtocol::Messages,
            item_type,
            ..
        } if input_only_media_type(item_type) => Some(item_type.as_str()),
        _ => None,
    };
    match kind {
        Some(kind) => Err(format!(
            "Messages responses cannot represent top-level {kind} content"
        )),
        None => Ok(()),
    }
}

pub(crate) fn validate_response_nodes(nodes: &[Node]) -> Result<(), String> {
    nodes.iter().try_for_each(validate_response_node)
}

fn merge_messages_media_metadata(
    block: &mut Value,
    metadata: &MediaMetadata,
    extra_body: &HashMap<String, Value>,
    document: bool,
) {
    let Some(obj) = block.as_object_mut() else {
        return;
    };
    merge_extra(obj, extra_body);
    for key in ["filename", "detail", "title", "context", "citations"] {
        obj.remove(key);
    }
    if document {
        if let Some(title) = &metadata.document_title {
            obj.insert("title".into(), json!(title));
        }
        if let Some(context) = &metadata.document_context {
            obj.insert("context".into(), json!(context));
        }
        if let Some(citations) = &metadata.document_citations {
            obj.insert("citations".into(), citations.clone());
        }
    }
}

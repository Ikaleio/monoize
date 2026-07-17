fn image_media_type_from_output_format(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

fn native_image_generation_call_body(payload: &Value) -> Option<Value> {
    match payload.get("type").and_then(Value::as_str) {
        Some("image_generation_call") => Some(payload.clone()),
        Some("image_generation.completed" | "response.image_generation.completed") => {
            let mut item = payload.as_object()?.clone();
            let result = item
                .get("result")
                .or_else(|| item.get("b64_json"))
                .and_then(Value::as_str)?
                .to_string();
            item.remove("b64_json");
            item.remove("sequence_number");
            item.insert("type".to_string(), json!("image_generation_call"));
            item.insert("status".to_string(), json!("completed"));
            item.insert("result".to_string(), Value::String(result));
            Some(Value::Object(item))
        }
        _ => None,
    }
}

fn image_node_from_image_generation_payload(payload: &Value) -> Option<Node> {
    let data = payload
        .get("b64_json")
        .or_else(|| payload.get("result"))
        .or_else(|| payload.get("partial_image_b64"))
        .and_then(|value| value.as_str())?
        .trim();
    if data.is_empty() {
        return None;
    }
    let mut extra_body = split_known_fields(
        payload.clone(),
        &[
            "type",
            "id",
            "b64_json",
            "result",
            "partial_image_b64",
            "output_format",
            "partial_image_index",
        ],
    );
    if let Some(native_body) = native_image_generation_call_body(payload) {
        extra_body.insert(
            RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
            native_body,
        );
    }
    Some(Node::Image {
        id: payload
            .get("id")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source: crate::urp::ImageSource::Base64 {
            media_type: image_media_type_from_output_format(
                payload
                    .get("output_format")
                    .and_then(|value| value.as_str()),
            )
            .to_string(),
            data: data.to_string(),
        },
        extra_body,
    })
}

fn image_generation_call_event_extra_body(payload: Value) -> HashMap<String, Value> {
    let provider_event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut extra_body = split_known_fields(payload, &["type", "sequence_number"]);
    if let Some(provider_event_type) = provider_event_type {
        extra_body.insert(
            "provider_event_type".to_string(),
            Value::String(provider_event_type),
        );
    }
    extra_body
}

fn map_image_generation_call_event(
    data_val: Value,
    output_index: u64,
    index_state: &mut ResponsesStreamIndexState,
) -> Vec<UrpStreamEvent> {
    let node_index = index_state.synthetic_node_index_for_output(output_index);
    if data_val.get("type").and_then(Value::as_str)
        == Some("response.image_generation_call.partial_image")
    {
        if let Some(node) = image_node_from_image_generation_payload(&data_val) {
            let extra_body = image_generation_call_event_extra_body(data_val);
            return vec![UrpStreamEvent::NodeDelta {
                node_index,
                delta: NodeDelta::Image {
                    source: match node {
                        Node::Image { source, .. } => source,
                        _ => unreachable!(),
                    },
                },
                usage: None,
                extra_body,
            }];
        }
    }
    vec![UrpStreamEvent::NodeDelta {
        node_index,
        delta: NodeDelta::ProviderItem {
            data: Value::Null,
        },
        usage: None,
        extra_body: image_generation_call_event_extra_body(data_val),
    }]
}

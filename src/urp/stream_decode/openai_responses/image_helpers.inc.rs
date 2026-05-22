fn image_media_type_from_output_format(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

fn image_node_from_image_generation_payload(payload: &Value) -> Option<Node> {
    let data = payload
        .get("b64_json")
        .or_else(|| payload.get("result"))
        .and_then(|value| value.as_str())?
        .trim();
    if data.is_empty() {
        return None;
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
        extra_body: split_known_fields(
            payload.clone(),
            &[
                "type",
                "id",
                "b64_json",
                "result",
                "output_format",
                "partial_image_index",
            ],
        ),
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
    vec![UrpStreamEvent::NodeDelta {
        node_index,
        delta: NodeDelta::ProviderItem {
            data: Value::Null,
        },
        usage: None,
        extra_body: image_generation_call_event_extra_body(data_val),
    }]
}

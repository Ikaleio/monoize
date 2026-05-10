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


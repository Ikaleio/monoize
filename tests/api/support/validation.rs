
fn maybe_forced_upstream_error(body: &Value) -> Option<axum::response::Response> {
    let status_u64 = body
        .get("force_upstream_error_status")
        .and_then(|v| v.as_u64())?;
    let status_u16 = u16::try_from(status_u64).ok()?;
    let status = StatusCode::from_u16(status_u16).ok()?;
    let code = body
        .get("force_upstream_error_code")
        .and_then(|v| v.as_str())
        .unwrap_or("forced_upstream_error");
    let message = body
        .get("force_upstream_error_message")
        .and_then(|v| v.as_str())
        .unwrap_or("forced upstream error");
    Some(
        (
            status,
            Json(json!({
                "error": {
                    "code": code,
                    "message": message
                }
            })),
        )
            .into_response(),
    )
}

fn maybe_reasoning_summary_validation_error(body: &Value) -> Option<axum::response::Response> {
    if body
        .get("require_reasoning_input_summary")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        return None;
    }
    let input = body.get("input").and_then(|v| v.as_array())?;
    let missing_summary_index = input.iter().position(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning")
            && item.get("summary").is_none()
    });
    if let Some(missing_index) = missing_summary_index {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": format!("Missing required parameter: 'input[{missing_index}].summary'."),
                    "type": "invalid_request_error",
                    "param": format!("input[{missing_index}].summary"),
                    "code": "missing_required_parameter"
                }
            })),
        )
            .into_response());
    }
    let unknown_source_index = input.iter().position(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning")
            && item.get("source").is_some()
    });
    if let Some(unknown_source_index) = unknown_source_index {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": format!("Unknown parameter: 'input[{unknown_source_index}].source'."),
                    "type": "invalid_request_error",
                    "param": format!("input[{unknown_source_index}].source"),
                    "code": "unknown_parameter"
                }
            })),
        )
            .into_response());
    }
    let unknown_text_index = input.iter().position(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("reasoning") && item.get("text").is_some()
    })?;
    Some(
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": format!("Unknown parameter: 'input[{unknown_text_index}].text'."),
                    "type": "invalid_request_error",
                    "param": format!("input[{unknown_text_index}].text"),
                    "code": "unknown_parameter"
                }
            })),
        )
            .into_response(),
    )
}

fn maybe_assistant_output_content_validation_error(
    body: &Value,
) -> Option<axum::response::Response> {
    if body
        .get("require_assistant_output_content_types")
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        return None;
    }

    let input = body.get("input").and_then(|v| v.as_array())?;
    let invalid_index = input.iter().enumerate().find_map(|(index, item)| {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            return None;
        }
        if item.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            return None;
        }
        let content = item.get("content").and_then(|v| v.as_array())?;
        let invalid_part = content.iter().position(|part| {
            !matches!(
                part.get("type").and_then(|v| v.as_str()),
                Some("output_text" | "refusal" | "output_image" | "output_file")
            )
        })?;
        Some((index, invalid_part))
    });

    let (message_index, content_index) = invalid_index?;
    Some((
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "message": format!("Invalid value: 'input_text'. Supported values are: 'output_text' and 'refusal'."),
                "type": "invalid_request_error",
                "param": format!("input[{message_index}].content[{content_index}]"),
                "code": "invalid_value"
            }
        })),
    )
        .into_response())
}

async fn maybe_forced_upstream_delay(body: &Value) {
    let Some(delay_ms) = body.get("force_upstream_delay_ms").and_then(|v| v.as_u64()) else {
        return;
    };
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}


use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_stream_done_sentinel, record_stream_terminal_event,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{FinishReason, ImageSource, Node, NodeHeader, OrdinaryRole, UrpStreamEvent};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_image_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    let mut started_response = false;
    let mut output = Vec::new();
    let mut next_node_index = 0u32;
    let idle_timeout = std::time::Duration::from_secs(120);
    let mut stream = upstream_resp.bytes_stream().eventsource();

    while let Some(ev) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                "upstream stream idle for 120s without data",
            )
        })?
    {
        let ev = ev.map_err(|err| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
                err.to_string(),
            )
        })?;
        if tx.is_closed() {
            break;
        }

        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }

        match ev.event.as_str() {
            "image_generation.partial_image" | "response.image_generation.partial_image" => {}
            "image_generation.completed" | "response.image_generation.completed" => {
                let data_val: Value = serde_json::from_str(&ev.data).map_err(|err| {
                    AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_decode_failed",
                        err.to_string(),
                    )
                })?;
                if let Some(node) = image_node_from_payload(&data_val) {
                    if !started_response {
                        tx.send(UrpStreamEvent::ResponseStart {
                            id: response_id.clone(),
                            model: urp.model.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await
                        .map_err(send_failed)?;
                        started_response = true;
                    }
                    let node_index = next_node_index;
                    next_node_index = next_node_index.saturating_add(1);
                    let extra_body = image_extra_body(&data_val);
                    tx.send(UrpStreamEvent::NodeStart {
                        node_index,
                        header: node_header(&node),
                        extra_body: extra_body.clone(),
                    })
                    .await
                    .map_err(send_failed)?;
                    tx.send(UrpStreamEvent::NodeDone {
                        node_index,
                        node: node.clone(),
                        usage: None,
                        extra_body,
                    })
                    .await
                    .map_err(send_failed)?;
                    output.push(node);
                }
            }
            "error" => {
                let message = serde_json::from_str::<Value>(&ev.data)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or(ev.data);
                tx.send(UrpStreamEvent::Error {
                    code: Some("upstream_image_error".to_string()),
                    message,
                    extra_body: HashMap::new(),
                })
                .await
                .map_err(send_failed)?;
                break;
            }
            _ => {}
        }
    }

    if started_response {
        tx.send(UrpStreamEvent::ResponseDone {
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            output,
            extra_body: HashMap::from([("id".to_string(), Value::String(response_id))]),
        })
        .await
        .map_err(send_failed)?;
    }
    record_stream_terminal_event(&runtime_metrics, "response.completed", Some("stop")).await;
    Ok(())
}

fn send_failed(err: mpsc::error::SendError<UrpStreamEvent>) -> AppError {
    AppError::new(
        StatusCode::BAD_GATEWAY,
        "stream_send_failed",
        err.to_string(),
    )
}

fn image_media_type(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

fn image_node_from_payload(payload: &Value) -> Option<Node> {
    let data = payload
        .get("b64_json")
        .or_else(|| payload.get("result"))
        .and_then(Value::as_str)?
        .trim();
    if data.is_empty() {
        return None;
    }
    Some(Node::Image {
        id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source: ImageSource::Base64 {
            media_type: image_media_type(payload.get("output_format").and_then(Value::as_str))
                .to_string(),
            data: data.to_string(),
        },
        extra_body: image_extra_body(payload),
    })
}

fn image_extra_body(payload: &Value) -> HashMap<String, Value> {
    let known = [
        "type",
        "id",
        "b64_json",
        "result",
        "output_format",
        "partial_image_index",
    ];
    payload
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, _)| !known.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn node_header(node: &Node) -> NodeHeader {
    match node {
        Node::Image { id, role, .. } => NodeHeader::Image {
            id: id.clone(),
            role: *role,
        },
        _ => NodeHeader::ProviderItem {
            id: None,
            role: OrdinaryRole::Assistant,
            item_type: "image_generation".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completed_payload_decodes_base64_image_node() {
        let node = image_node_from_payload(&json!({
            "type": "image_generation.completed",
            "id": "ig_1",
            "b64_json": "QUJD",
            "output_format": "webp"
        }))
        .expect("image node");

        assert!(matches!(
            node,
            Node::Image {
                id: Some(id),
                role: OrdinaryRole::Assistant,
                source: ImageSource::Base64 { media_type, data },
                ..
            } if id == "ig_1" && media_type == "image/webp" && data == "QUJD"
        ));
    }

    #[test]
    fn partial_image_event_can_be_ignored() {
        assert!(
            image_node_from_payload(&json!({
                "type": "image_generation.partial_image",
                "partial_image_index": 0,
                "b64_json": ""
            }))
            .is_none()
        );
    }
}

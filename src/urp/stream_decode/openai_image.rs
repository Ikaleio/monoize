use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_stream_done_sentinel, record_stream_terminal_event,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, ImageSource, Node, NodeDelta, NodeHeader, OrdinaryRole, ProviderProtocol,
    UrpStreamEvent, Usage,
};
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
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    let mut started_response = false;
    let mut output = Vec::new();
    let mut next_node_index = 0u32;
    // OIU-S2b: partial frames and the completed node of one generation share
    // one node index; the completed event consumes the pending index so the
    // next generation allocates a fresh one.
    let mut pending_node_index: Option<u32> = None;
    let mut usage: Option<Usage> = None;
    let idle_timeout = std::time::Duration::from_millis(idle_timeout_ms.max(1));
    let mut stream = upstream_resp.bytes_stream().eventsource();

    while let Some(ev) = tokio::time::timeout(idle_timeout, stream.next())
        .await
        .map_err(|_| {
            AppError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
                format!("upstream stream idle for {idle_timeout_ms}ms without data"),
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
        mark_stream_ttfb_if_needed(started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }

        let event_name = resolve_event_name(&ev.event, &ev.data);
        match event_name.as_str() {
            "image_generation.partial_image"
            | "image_edit.partial_image"
            | "response.image_generation.partial_image" => {
                let data_val: Value = serde_json::from_str(&ev.data).map_err(|err| {
                    AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_decode_failed",
                        err.to_string(),
                    )
                })?;
                let Some(source) = image_source_from_payload(&data_val) else {
                    continue;
                };
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
                let node_index = *pending_node_index.get_or_insert_with(|| {
                    let index = next_node_index;
                    next_node_index = next_node_index.saturating_add(1);
                    index
                });
                tx.send(UrpStreamEvent::NodeDelta {
                    node_index,
                    delta: NodeDelta::Image { source },
                    usage: None,
                    extra_body: partial_image_extra_body(&event_name, &data_val),
                })
                .await
                .map_err(send_failed)?;
            }
            "image_generation.completed"
            | "image_edit.completed"
            | "response.image_generation.completed" => {
                let data_val: Value = serde_json::from_str(&ev.data).map_err(|err| {
                    AppError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream_stream_decode_failed",
                        err.to_string(),
                    )
                })?;
                if let Some(parsed) = data_val
                    .get("usage")
                    .and_then(crate::urp::decode::openai_image::parse_image_usage)
                {
                    usage = Some(parsed);
                }
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
                    let node_index = pending_node_index.take().unwrap_or_else(|| {
                        let index = next_node_index;
                        next_node_index = next_node_index.saturating_add(1);
                        index
                    });
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
            usage,
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

/// OIU-S1a: the SSE `event` field wins; frames without an explicit event name
/// (eventsource default `message`) fall back to the JSON `type` field.
fn resolve_event_name(sse_event: &str, data: &str) -> String {
    if !sse_event.is_empty() && sse_event != "message" {
        return sse_event.to_string();
    }
    serde_json::from_str::<Value>(data)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| sse_event.to_string())
}

fn image_media_type(output_format: Option<&str>) -> &'static str {
    match output_format.unwrap_or("png") {
        "webp" => "image/webp",
        "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

fn image_source_from_payload(payload: &Value) -> Option<ImageSource> {
    let data = payload
        .get("b64_json")
        .or_else(|| payload.get("result"))
        .and_then(Value::as_str)?
        .trim();
    if data.is_empty() {
        return None;
    }
    Some(ImageSource::Base64 {
        media_type: image_media_type(payload.get("output_format").and_then(Value::as_str))
            .to_string(),
        data: data.to_string(),
    })
}

fn image_node_from_payload(payload: &Value) -> Option<Node> {
    let source = image_source_from_payload(payload)?;
    Some(Node::Image {
        id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source,
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
        "usage",
    ];
    payload
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, _)| {
                    !crate::urp::decode::is_internal_extra_key(key)
                        && !known.contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// OIU-S2: partial-image `NodeDelta` extra fields keep `partial_image_index`
/// and `output_format` (unlike terminal image nodes, where they are header
/// data) so downstream encoders can rebuild the wire event.
fn partial_image_extra_body(event_name: &str, payload: &Value) -> HashMap<String, Value> {
    let excluded = ["type", "b64_json", "result"];
    let mut extra_body: HashMap<String, Value> = payload
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, _)| {
                    !crate::urp::decode::is_internal_extra_key(key)
                        && !excluded.contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    extra_body.insert(
        "provider_event_type".to_string(),
        Value::String(event_name.to_string()),
    );
    extra_body
}

fn node_header(node: &Node) -> NodeHeader {
    match node {
        Node::Image { id, role, .. } => NodeHeader::Image {
            id: id.clone(),
            role: *role,
        },
        _ => NodeHeader::ProviderItem {
            id: None,
            origin_protocol: ProviderProtocol::OpenaiImage,
            role: OrdinaryRole::Assistant,
            item_type: "image_generation".to_string(),
        },
    }
}

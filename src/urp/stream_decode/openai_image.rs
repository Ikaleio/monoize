use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_stream_done_sentinel, record_stream_terminal_event,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, ImageSource, Node, NodeDelta, NodeHeader, OrdinaryRole, TerminalNode,
    UrpStreamEvent, Usage, is_internal_key,
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
    mut started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut decoder = ImageStreamDecoder::new(tx, urp.model.clone());
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
                crate::error::sse_error_chain(&err),
            )
        })?;
        mark_stream_ttfb_if_needed(&mut started_at, &runtime_metrics).await;
        if ev.data.trim() == "[DONE]" {
            record_stream_done_sentinel(&runtime_metrics).await;
            break;
        }

        let event_name = resolve_event_name(&ev.event, &ev.data);
        match event_name.as_str() {
            "image_generation.partial_image"
            | "image_edit.partial_image"
            | "response.image_generation.partial_image" => {
                decoder
                    .on_partial_image(&event_name, &parse_payload(&ev.data)?)
                    .await?;
            }
            "image_generation.completed"
            | "image_edit.completed"
            | "response.image_generation.completed" => {
                decoder.on_completed(&parse_payload(&ev.data)?).await?;
            }
            "error" => {
                decoder.on_error(ev.data).await?;
                return Ok(());
            }
            _ => {}
        }
    }

    decoder.finish().await?;
    record_stream_terminal_event(&runtime_metrics, "response.completed", Some("stop")).await;
    Ok(())
}

/// Cross-event state of one upstream Images API stream.
struct ImageStreamDecoder {
    tx: mpsc::Sender<UrpStreamEvent>,
    response_id: String,
    model: String,
    started_response: bool,
    output: Vec<TerminalNode>,
    next_node_index: u32,
    // OIU-S2b: partial frames and the completed node of one generation share
    // one node index; the completed event consumes the pending index so the
    // next generation allocates a fresh one.
    pending_node_index: Option<u32>,
    usage: Option<Usage>,
}

impl ImageStreamDecoder {
    fn new(tx: mpsc::Sender<UrpStreamEvent>, model: String) -> Self {
        Self {
            tx,
            response_id: format!("resp_{}", uuid::Uuid::new_v4().simple()),
            model,
            started_response: false,
            output: Vec::new(),
            next_node_index: 0,
            pending_node_index: None,
            usage: None,
        }
    }

    async fn send(&self, event: UrpStreamEvent) -> AppResult<()> {
        self.tx.send(event).await.map_err(send_failed)
    }

    async fn ensure_started(&mut self, payload: &Value) -> AppResult<()> {
        if self.started_response {
            return Ok(());
        }
        self.started_response = true;
        self.send(UrpStreamEvent::ResponseStart {
            usage: None,
            id: self.response_id.clone(),
            model: self.model.clone(),
            created_at: payload.get("created_at").and_then(Value::as_i64),
            extra_body: HashMap::new(),
        })
        .await
    }

    fn allocate_node_index(&mut self) -> u32 {
        let index = self.next_node_index;
        self.next_node_index = self.next_node_index.saturating_add(1);
        index
    }

    async fn on_partial_image(&mut self, event_name: &str, payload: &Value) -> AppResult<()> {
        let Some(source) = image_source_from_payload(payload) else {
            return Ok(());
        };
        self.ensure_started(payload).await?;
        let node_index = match self.pending_node_index {
            Some(index) => index,
            None => {
                let index = self.allocate_node_index();
                self.pending_node_index = Some(index);
                index
            }
        };
        self.send(UrpStreamEvent::NodeDelta {
            node_index,
            delta: NodeDelta::Image { source },
            usage: None,
            extra_body: partial_image_extra_body(event_name, payload),
        })
        .await
    }

    async fn on_completed(&mut self, payload: &Value) -> AppResult<()> {
        if let Some(parsed) = payload
            .get("usage")
            .and_then(crate::urp::decode::openai_image::parse_image_usage)
        {
            self.usage = Some(parsed);
        }
        let Some(node) = image_node_from_payload(payload) else {
            return Ok(());
        };
        self.ensure_started(payload).await?;
        let node_index = match self.pending_node_index.take() {
            Some(index) => index,
            None => self.allocate_node_index(),
        };
        let extra_body = image_extra_body(payload);
        self.send(UrpStreamEvent::NodeStart {
            node_index,
            header: NodeHeader::from(&node),
            extra_body: extra_body.clone(),
        })
        .await?;
        self.send(UrpStreamEvent::NodeDone {
            node_index,
            node: node.clone(),
            usage: None,
            extra_body,
        })
        .await?;
        self.output.push(TerminalNode::streamed(node_index, node));
        Ok(())
    }

    async fn on_error(&self, data: String) -> AppResult<()> {
        let value = serde_json::from_str::<Value>(&data).unwrap_or(Value::Null);
        let error = value.get("error").unwrap_or(&value);
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("upstream_image_error")
            .to_string();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(data);
        self.send(UrpStreamEvent::Error {
            code: Some(code),
            message,
            extra_body: HashMap::new(),
        })
        .await
    }

    async fn finish(self) -> AppResult<()> {
        if self.output.is_empty() {
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_stream_missing_terminal",
                "upstream image stream ended without a completed image",
            ));
        }
        // A completed image always opened the response, so `ResponseStart` precedes this.
        let Self {
            tx,
            response_id,
            output,
            usage,
            ..
        } = self;
        tx.send(UrpStreamEvent::ResponseDone {
            outcome: None,
            finish_reason: Some(FinishReason::Stop),
            usage,
            output,
            extra_body: HashMap::from([("id".to_string(), Value::String(response_id))]),
        })
        .await
        .map_err(send_failed)
    }
}

fn parse_payload(data: &str) -> AppResult<Value> {
    serde_json::from_str(data).map_err(|err| {
        AppError::new(
            StatusCode::BAD_GATEWAY,
            "upstream_stream_decode_failed",
            err.to_string(),
        )
    })
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
        media_type: crate::urp::decode::openai_image::image_media_type(
            payload,
            payload.get("output_format").and_then(Value::as_str),
        ),
        data: data.to_string(),
    })
}

fn image_node_from_payload(payload: &Value) -> Option<Node> {
    let source = image_source_from_payload(payload)?;
    let reported_mime = crate::urp::decode::openai_image::reported_image_mime(payload);
    let mut image_generation =
        crate::urp::ImageGenerationMetadata::from_object(payload.as_object()?);
    crate::urp::decode::openai_image::apply_reported_output_format(
        &mut image_generation,
        reported_mime.as_deref(),
    );
    let mut extra_body = image_extra_body(payload);
    extra_body.insert(
        crate::urp::RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY.to_string(),
        Value::Object(Default::default()),
    );
    Some(Node::Image {
        metadata: crate::urp::MediaMetadata {
            media_type: reported_mime,
            image_generation,
            ..Default::default()
        },

        id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(crate::urp::synthetic_provider_item_id())),
        role: OrdinaryRole::Assistant,
        source,
        extra_body,
    })
}

fn image_extra_body(payload: &Value) -> HashMap<String, Value> {
    let known = [
        "type",
        "id",
        "b64_json",
        "result",
        "media_type",
        "mime_type",
        "output_format",
        "partial_image_index",
        "usage",
    ];
    payload
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, _)| {
                    !is_internal_key(key)
                        && !known.contains(&key.as_str())
                        && !crate::urp::ImageGenerationMetadata::KEYS.contains(&key.as_str())
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
    let excluded = ["type", "b64_json", "result", "media_type", "mime_type"];
    let mut extra_body: HashMap<String, Value> = payload
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, _)| !is_internal_key(key) && !excluded.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    extra_body.insert(
        "provider_event_type".to_string(),
        Value::String(event_name.to_string()),
    );
    if !extra_body.contains_key("output_format") {
        let mut generation = crate::urp::ImageGenerationMetadata::default();
        crate::urp::decode::openai_image::apply_reported_output_format(
            &mut generation,
            crate::urp::decode::openai_image::reported_image_mime(payload).as_deref(),
        );
        if let Some(format) = generation.output_format {
            extra_body.insert("output_format".to_string(), Value::String(format));
        }
    }
    extra_body
}

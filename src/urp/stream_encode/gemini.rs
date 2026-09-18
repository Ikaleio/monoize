use crate::error::{AppError, AppResult};
use crate::urp::encode::gemini::{
    encode_response_checked, encode_response_node_parts, is_prompt_block_response,
};
use crate::urp::{Node, UrpResponse, UrpStreamEvent};
use axum::http::StatusCode;
use axum::response::sse::Event;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

/// Encodes complete canonical nodes and reconciles terminal-only output without duplicate Parts.
/// Gemini cannot retract an emitted Part. A conflicting terminal replacement returns an error.
pub struct GeminiStreamEncoder {
    id: String,
    model: String,
    next_index: u32,
    completed: BTreeMap<u32, Node>,
    emitted: BTreeMap<u32, Vec<Value>>,
    terminal: bool,
}

impl GeminiStreamEncoder {
    pub fn new(model: &str) -> Self {
        Self {
            id: String::new(),
            model: model.into(),
            next_index: 0,
            completed: BTreeMap::new(),
            emitted: BTreeMap::new(),
            terminal: false,
        }
    }

    pub fn push_event(&mut self, event: UrpStreamEvent) -> Result<Vec<Value>, String> {
        if self.terminal {
            return Ok(Vec::new());
        }
        match event {
            UrpStreamEvent::ResponseStart { id, .. } => {
                self.id = id;
                Ok(Vec::new())
            }
            UrpStreamEvent::NodeDone {
                node_index, node, ..
            } => {
                if node_index < self.next_index {
                    return self
                        .emit_node(node_index, &node)
                        .map(|frame| frame.into_iter().collect());
                }
                self.completed.insert(node_index, node);
                let mut frames = Vec::new();
                while let Some(node) = self.completed.remove(&self.next_index) {
                    let index = self.next_index;
                    self.next_index += 1;
                    if !matches!(node, Node::Refusal { .. }) {
                        frames.extend(self.emit_node(index, &node)?);
                    }
                }
                Ok(frames)
            }
            UrpStreamEvent::ResponseDone {
                finish_reason,
                usage,
                output,
                extra_body,
            } => {
                if self
                    .emitted
                    .keys()
                    .any(|index| *index as usize >= output.len())
                {
                    return Err("Gemini cannot delete a Part that was already emitted".into());
                }
                let response = UrpResponse {
                    id: self.id.clone(),
                    model: self.model.clone(),
                    created_at: None,
                    output,
                    finish_reason,
                    usage,
                    extra_body,
                };
                let mut terminal = encode_response_checked(&response, &self.model)?;
                let mut frames = Vec::new();
                for (index, node) in response
                    .output
                    .iter()
                    .enumerate()
                    .filter(|_| !is_prompt_block_response(&response))
                {
                    if let Some(frame) = self.emit_node(index as u32, node)? {
                        frames.push(frame);
                    }
                }
                if let Some(candidate) = terminal["candidates"]
                    .as_array_mut()
                    .and_then(|values| values.first_mut())
                    .and_then(Value::as_object_mut)
                {
                    candidate.remove("content");
                }
                frames.push(terminal);
                self.terminal = true;
                Ok(frames)
            }
            UrpStreamEvent::Error { code, message, .. } => {
                self.terminal = true;
                Ok(vec![json!({"error":{"code":code,"message":message}})])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn emit_node(&mut self, index: u32, node: &Node) -> Result<Option<Value>, String> {
        let parts = encode_response_node_parts(node)?;
        if parts.is_empty() {
            return if self.emitted.contains_key(&index) {
                Err("Gemini cannot delete a Part that was already emitted".into())
            } else {
                Ok(None)
            };
        }
        let mut fragments = parts.clone();
        if let Some(previous) = self.emitted.get(&index) {
            if *previous == parts {
                return Ok(None);
            }
            if self.emitted.keys().any(|emitted| *emitted > index) {
                return Err("Gemini cannot modify a Part before a later emitted Part".into());
            }
            if previous.len() != 1 || parts.len() != 1 {
                return Err("Gemini cannot replace Parts that were already emitted".into());
            }
            let previous = &previous[0];
            let part = &parts[0];
            let prefix = previous.get("text").and_then(Value::as_str);
            let text = part.get("text").and_then(Value::as_str);
            let mut old_shape = previous.clone();
            let mut new_shape = part.clone();
            if let Some(obj) = old_shape.as_object_mut() {
                obj.remove("text");
            }
            if let Some(obj) = new_shape.as_object_mut() {
                obj.remove("text");
            }
            if let (Some(prefix), Some(text)) = (prefix, text) {
                if old_shape == new_shape
                    && part.get("thoughtSignature").is_none()
                    && text.starts_with(prefix)
                {
                    fragments[0]["text"] = json!(&text[prefix.len()..]);
                } else {
                    return Err("Gemini cannot replace a Part that was already emitted".into());
                }
            } else {
                return Err("Gemini cannot replace a Part that was already emitted".into());
            }
        } else if self.emitted.keys().any(|emitted| *emitted > index) {
            return Err("Gemini cannot insert a Part before a later emitted Part".into());
        }
        self.emitted.insert(index, parts);
        Ok(Some(json!({"responseId":self.id,"modelVersion":self.model,
            "candidates":[{"index":0,"content":{"role":"model","parts":fragments}}]})))
    }
}

/// Writes Gemini data-only SSE frames. Completion requires a canonical terminal event.
pub async fn encode_urp_stream_as_gemini(
    mut rx: mpsc::Receiver<UrpStreamEvent>,
    tx: mpsc::Sender<Event>,
    model: &str,
) -> AppResult<()> {
    let mut encoder = GeminiStreamEncoder::new(model);
    while let Some(event) = rx.recv().await {
        let frames = match encoder.push_event(event) {
            Ok(frames) => frames,
            Err(message) => {
                let body = crate::urp::media::error_body(&message);
                crate::urp::stream_helpers::send_plain_sse_data(&tx, body.to_string()).await?;
                return Err(AppError::new(
                    StatusCode::BAD_GATEWAY,
                    "stream_encode_failed",
                    message,
                )
                .with_downstream_stream_terminal_sent(!tx.is_closed()));
            }
        };
        for frame in frames {
            crate::urp::stream_helpers::send_plain_sse_data(&tx, frame.to_string()).await?;
        }
    }
    if !encoder.terminal {
        let message = "Gemini stream has no canonical terminal event";
        crate::urp::stream_helpers::send_plain_sse_data(
            &tx,
            crate::urp::media::error_body(message).to_string(),
        )
        .await?;
        return Err(
            AppError::new(StatusCode::BAD_GATEWAY, "stream_encode_failed", message)
                .with_downstream_stream_terminal_sent(!tx.is_closed()),
        );
    }
    Ok(())
}

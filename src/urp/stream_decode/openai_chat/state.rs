//! Cross-chunk state of one upstream Chat Completions stream and its URP event emitters.

use super::details::DetailNode;
use super::reasoning::ScalarReasoning;
use super::text::{AudioState, RefusalState, TextState};
use super::tool_calls::ToolCalls;
use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    CHAT_CHOICE_EXTRA_BODY_KEY, CHAT_CHUNK_EXTRA_KEY, CHAT_DELTA_EXTRA_BODY_KEY, FinishReason,
    Node, NodeDelta, NodeHeader, UrpStreamEvent, Usage,
};
use axum::http::StatusCode;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};

/// What the driver loop does after one upstream SSE event.
pub(super) enum Flow {
    Continue,
    /// `[DONE]` arrived after a terminal `finish_reason`: complete the response.
    Done,
    /// A terminal `Error` event was emitted; nothing follows it.
    Stop,
}

pub(super) struct ChatStreamState<'a> {
    pub(super) tx: mpsc::Sender<UrpStreamEvent>,
    pub(super) runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    /// Request start, until the first upstream event records the time to first byte.
    pub(super) started_at: Option<Instant>,
    pub(super) model: &'a str,
    /// MIME type of the requested audio output format, applied to generated audio.
    pub(super) audio_mime: Option<&'static str>,
    /// Visible output bytes not yet reported; they ride along with the next metrics update.
    pub(super) pending_visible_output_bytes: u64,

    pub(super) response_id: String,
    pub(super) created_at: Option<i64>,
    /// DC6a: unknown top-level members of the first chunk that carries any, for `ResponseStart`.
    pub(super) chunk_extra: Option<Map<String, Value>>,
    pub(super) response_started: bool,
    pub(super) next_node_index: u32,
    pub(super) latest_usage: Option<Usage>,
    pub(super) finish_reason: Option<FinishReason>,
    /// A non-error `finish_reason` arrived, so the stream may complete.
    pub(super) terminal_seen: bool,
    pub(super) terminal_extra: HashMap<String, Value>,
    /// Unknown delta members (and choice members under `CHAT_CHOICE_EXTRA_BODY_KEY`) that no
    /// event carried yet. The next delta-bearing event takes them, in any later chunk.
    pub(super) delta_extra: Map<String, Value>,
    /// Assistant message `phase`, from the first delta or terminal message that names one.
    pub(super) phase: Option<String>,

    pub(super) text: TextState,
    pub(super) reasoning: ScalarReasoning,
    /// One node per Chat `reasoning_details[]` entry, in start order.
    pub(super) details: Vec<DetailNode>,
    /// PC7h: `reasoning.encrypted` entries whose data has not arrived yet.
    pub(super) held_placeholders: Vec<Value>,
    pub(super) tool_calls: ToolCalls,
    /// Media and unrecognized content blocks, each a complete node.
    pub(super) provider_items: Vec<(u32, Node)>,
    pub(super) refusal: Option<RefusalState>,
    pub(super) audio: Option<AudioState>,
    /// PC7h: completed text and scalar reasoning segments, already sent with `NodeDone`.
    pub(super) closed: Vec<(u32, Node)>,
}

impl<'a> ChatStreamState<'a> {
    pub(super) fn new(
        urp: &'a HandlerUrpRequest,
        tx: mpsc::Sender<UrpStreamEvent>,
        started_at: Option<Instant>,
        runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    ) -> Self {
        Self {
            tx,
            runtime_metrics,
            started_at,
            model: &urp.model,
            audio_mime: urp
                .audio_output_format
                .as_deref()
                .and_then(crate::urp::media::audio_mime_for_format),
            pending_visible_output_bytes: 0,
            response_id: format!("resp_{}", uuid::Uuid::new_v4()),
            created_at: None,
            chunk_extra: None,
            response_started: false,
            next_node_index: 0,
            latest_usage: None,
            finish_reason: None,
            terminal_seen: false,
            terminal_extra: HashMap::new(),
            delta_extra: Map::new(),
            phase: None,
            text: TextState::default(),
            reasoning: ScalarReasoning::default(),
            details: Vec::new(),
            held_placeholders: Vec::new(),
            tool_calls: ToolCalls::default(),
            provider_items: Vec::new(),
            refusal: None,
            audio: None,
            closed: Vec::new(),
        }
    }

    pub(super) async fn send(&self, event: UrpStreamEvent) -> AppResult<()> {
        self.tx.send(event).await.map_err(|error| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "stream_send_failed",
                error.to_string(),
            )
        })
    }

    /// Emits `ResponseStart` once, with the native creation time and the DC6a chunk members.
    pub(super) async fn start_response(
        &mut self,
        mut extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        if self.response_started {
            return Ok(());
        }
        if let Some(chunk_extra) = self.chunk_extra.take() {
            extra_body.insert(CHAT_CHUNK_EXTRA_KEY.to_string(), Value::Object(chunk_extra));
        }
        self.send(UrpStreamEvent::ResponseStart {
            usage: None,
            id: self.response_id.clone(),
            model: self.model.to_string(),
            created_at: self.created_at,
            extra_body,
        })
        .await?;
        self.response_started = true;
        Ok(())
    }

    /// Starts the response if needed, then announces a node at the next index.
    pub(super) async fn start_node(
        &mut self,
        header: NodeHeader,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<u32> {
        self.start_response(HashMap::new()).await?;
        let node_index = self.next_node_index;
        self.next_node_index += 1;
        self.send(UrpStreamEvent::NodeStart {
            node_index,
            header,
            extra_body,
        })
        .await?;
        Ok(node_index)
    }

    pub(super) async fn send_delta(
        &self,
        node_index: u32,
        delta: NodeDelta,
        extra_body: HashMap<String, Value>,
    ) -> AppResult<()> {
        self.send(UrpStreamEvent::NodeDelta {
            node_index,
            delta,
            usage: None,
            extra_body,
        })
        .await
    }

    pub(super) async fn send_done(&self, node_index: u32, node: Node) -> AppResult<()> {
        self.send(UrpStreamEvent::NodeDone {
            node_index,
            node,
            usage: None,
            extra_body: HashMap::new(),
        })
        .await
    }

    /// The pending delta members as event extras: delta members under
    /// `CHAT_DELTA_EXTRA_BODY_KEY`, choice members under `CHAT_CHOICE_EXTRA_BODY_KEY`.
    pub(super) fn take_delta_extra(&mut self) -> HashMap<String, Value> {
        let mut delta_extra = std::mem::take(&mut self.delta_extra);
        let choice_extra = delta_extra.remove(CHAT_CHOICE_EXTRA_BODY_KEY);
        let mut event_extra = HashMap::new();
        if !delta_extra.is_empty() {
            event_extra.insert(
                CHAT_DELTA_EXTRA_BODY_KEY.to_string(),
                Value::Object(delta_extra),
            );
        }
        if let Some(choice_extra) = choice_extra {
            event_extra.insert(CHAT_CHOICE_EXTRA_BODY_KEY.to_string(), choice_extra);
        }
        event_extra
    }

    /// The index of the most recently announced node.
    pub(super) fn latest_node_index(&self) -> Option<u32> {
        self.next_node_index.checked_sub(1)
    }
}

use crate::error::AppResult;
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_stream_done_sentinel, record_stream_terminal_event,
    record_visible_output_delta,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::payload::sse::Frames;
use crate::urp::{
    FinishReason, Node, NodeDelta, NodeHeader, OrdinaryRole, TerminalNode, UrpStreamEvent,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// Replicate streams carry one assistant text node at this index.
const TEXT_NODE_INDEX: u32 = 0;

pub(crate) async fn stream_replicate_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    mut started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    idle_timeout_ms: u64,
) -> AppResult<()> {
    let mut decoder = ReplicateStreamDecoder::new(tx, urp.model.clone());
    let mut stream = Frames::new(upstream_resp, idle_timeout_ms, true);

    while let Some(ev) = stream.next().await.map_err(|error| error.into_app())? {
        mark_stream_ttfb_if_needed(&mut started_at, &runtime_metrics).await;

        let crate::payload::sse::FrameData::Text(data) = ev.data else {
            unreachable!()
        };
        match ev.event.as_str() {
            "output" => {
                record_visible_output_delta(&runtime_metrics, &data).await;
                decoder.on_output(data).await;
            }
            "error" => decoder.on_error(data).await,
            "done" => {
                record_stream_done_sentinel(&runtime_metrics).await;
                decoder.on_done();
                break;
            }
            _ => {}
        }
    }

    decoder.finish().await;
    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

/// Cross-event state of one Replicate prediction stream. Send failures are ignored: a closed
/// receiver means the downstream is gone and the remaining events have no consumer.
struct ReplicateStreamDecoder {
    tx: mpsc::Sender<UrpStreamEvent>,
    response_id: String,
    model: String,
    started_response: bool,
    text_started: bool,
    output_text: String,
    had_error: bool,
    finish_reason: Option<FinishReason>,
}

impl ReplicateStreamDecoder {
    fn new(tx: mpsc::Sender<UrpStreamEvent>, model: String) -> Self {
        Self {
            tx,
            response_id: format!("resp_{}", uuid::Uuid::new_v4()),
            model,
            started_response: false,
            text_started: false,
            output_text: String::new(),
            had_error: false,
            finish_reason: None,
        }
    }

    async fn send(&self, event: UrpStreamEvent) {
        let _ = self.tx.send(event).await;
    }

    async fn on_output(&mut self, data: String) {
        if !self.started_response {
            self.started_response = true;
            self.send(UrpStreamEvent::ResponseStart {
                usage: None,
                id: self.response_id.clone(),
                model: self.model.clone(),
                created_at: None,
                extra_body: HashMap::new(),
            })
            .await;
        }
        if !self.text_started {
            self.text_started = true;
            self.send(UrpStreamEvent::NodeStart {
                node_index: TEXT_NODE_INDEX,
                header: NodeHeader::Text {
                    signature: None,
                    citations: Vec::new(),
                    id: None,
                    role: OrdinaryRole::Assistant,
                    phase: None,
                },
                extra_body: HashMap::new(),
            })
            .await;
        }
        self.output_text.push_str(&data);
        self.send(UrpStreamEvent::NodeDelta {
            node_index: TEXT_NODE_INDEX,
            delta: NodeDelta::Text {
                logprobs: None,
                signature: None,
                citations: Vec::new(),
                content: data,
            },
            usage: None,
            extra_body: HashMap::new(),
        })
        .await;
    }

    async fn on_error(&mut self, data: String) {
        self.had_error = true;
        self.send(UrpStreamEvent::Error {
            code: Some("replicate_error".to_string()),
            message: data,
            extra_body: HashMap::new(),
        })
        .await;
    }

    fn on_done(&mut self) {
        self.finish_reason = Some(if self.had_error {
            FinishReason::Other
        } else {
            FinishReason::Stop
        });
    }

    async fn finish(mut self) {
        let mut output = Vec::new();
        if self.text_started {
            let node = Node::Text {
                logprobs: None,
                citations: Vec::new(),
                signature: None,
                id: None,
                role: OrdinaryRole::Assistant,
                content: std::mem::take(&mut self.output_text),
                phase: None,
                extra_body: HashMap::new(),
                cache_breakpoint: None,
            };
            self.send(UrpStreamEvent::NodeDone {
                node_index: TEXT_NODE_INDEX,
                node: node.clone(),
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
            output.push(TerminalNode::streamed(TEXT_NODE_INDEX, node));
        }
        if self.started_response {
            self.send(UrpStreamEvent::ResponseDone {
                outcome: None,
                finish_reason: self.finish_reason,
                usage: None,
                output,
                extra_body: HashMap::new(),
            })
            .await;
        }
    }
}

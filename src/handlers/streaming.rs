use super::*;
use crate::error::ForwardedHeaders;
use crate::urp::stream_decode::stream_upstream_to_urp_events;
use crate::urp::stream_encode::{StreamEncodeOptions, encode_urp_stream, replay_urp_response};
use futures_util::StreamExt;

mod errors;
mod forward;
mod live;
mod live_settle;
mod live_task;
mod stages;
mod synthetic;

pub(super) use errors::normalize_outcome_error;
use errors::*;
pub(super) use forward::forward_stream_typed;
use forward::*;
use live::*;
use live_settle::*;
pub(super) use stages::retain_reconstructed_urp_response;
use stages::*;
use synthetic::*;

enum StreamUpstreamSource {
    Http(reqwest::Response),
    ResponsesWebSocket {
        source: Box<crate::upstream_websocket::ResponsesWsTurnSource>,
        turn: crate::upstream_websocket::ResponsesWsTurn,
    },
    /// WS2c/WS6: synthetic Responses frames for a warmup without an upstream.
    LocalFrames(Vec<String>),
}

type ForwardEventStream = futures_util::stream::Map<
    tokio_stream::wrappers::ReceiverStream<Event>,
    fn(Event) -> Result<Event, std::convert::Infallible>,
>;

fn event_ok(event: Event) -> Result<Event, std::convert::Infallible> {
    Ok(event)
}

fn receiver_event_stream(rx: mpsc::Receiver<Event>) -> ForwardEventStream {
    tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(event_ok as fn(Event) -> Result<Event, std::convert::Infallible>)
}

/// WS5a: the final encoded body of a generated Responses attempt requests
/// encrypted reasoning, whatever the request transforms did to `include`.
fn enforce_ws_encrypted_include(
    turn: Option<&crate::upstream_websocket::ResponsesWsTurn>,
    provider_type: ProviderType,
    body: &mut Value,
) {
    if provider_type != ProviderType::Responses || turn.is_none_or(|turn| turn.warmup) {
        return;
    }
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let include = object.entry("include").or_insert(Value::Null);
    if !include.is_array() {
        *include = Value::Array(Vec::new());
    }
    if let Some(values) = include.as_array_mut()
        && !values
            .iter()
            .any(|value| value.as_str() == Some("reasoning.encrypted_content"))
    {
        values.push(Value::String("reasoning.encrypted_content".to_string()));
    }
}

/// FP4h: the single transition from "no downstream response yet" to committed
/// SSE headers. Cloned into the forwarding task and every attempt.
#[derive(Clone)]
pub(super) struct StreamCommit {
    sender:
        Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<AppResult<ForwardedHeaders>>>>>,
    downstream_gone: Arc<std::sync::atomic::AtomicBool>,
}

/// RL1h-1: sets the flag when the waiting handler or the returned response body
/// is dropped. The forwarding task feeds an internal channel, whose open state
/// says nothing about the real client.
struct DownstreamLifetime(Arc<std::sync::atomic::AtomicBool>);

impl Drop for DownstreamLifetime {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl StreamCommit {
    /// RL1h-1: whether the real downstream response body has gone away.
    fn downstream_gone(&self) -> bool {
        self.downstream_gone
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Commits the SSE headers. Later calls have no effect (SE1d).
    fn accept(&self, headers: ForwardedHeaders) {
        if let Some(tx) = self.take_sender() {
            let _ = tx.send(Ok(headers));
        }
    }

    /// Returns `err` to the caller when the headers were already committed,
    /// so it can render the in-stream error instead (SE2).
    fn reject(&self, err: AppError) -> Option<AppError> {
        match self.take_sender() {
            Some(tx) => {
                let _ = tx.send(Err(err));
                None
            }
            None => Some(err),
        }
    }

    fn take_sender(&self) -> Option<tokio::sync::oneshot::Sender<AppResult<ForwardedHeaders>>> {
        self.sender
            .lock()
            .expect("commit lock is never poisoned")
            .take()
    }
}

#[cfg(test)]
#[path = "stream_terminal_tests.rs"]
mod stream_terminal_tests;

/// SE2: the in-stream error of a failure after `COMMIT` and before any
/// protocol event. `[DONE]` belongs only to Chat Completions.
fn precommitted_error_stream(
    downstream: DownstreamProtocol,
    err: AppError,
    request_id: Option<String>,
) -> ForwardEventStream {
    let (tx, rx) = mpsc::channel::<Event>(8);
    tokio::spawn(async move {
        // `err` already holds client text: RTA-8 or a local error.
        emit_stream_error_if_needed(downstream, &err, request_id.as_deref(), false, &tx, None)
            .await;
    });
    receiver_event_stream(rx)
}

/// FP4e/FP4h: runs `forwarding` in its own task and answers once the first
/// upstream attempt is accepted or the request has failed. A failure before
/// the commit becomes a native JSON error; after it, an in-stream error.
pub(super) async fn commit_forward_event_stream<F, S>(
    downstream: DownstreamProtocol,
    request_id: Option<String>,
    keep_alive: KeepAlive,
    forwarding: impl FnOnce(StreamCommit) -> F,
) -> AppResult<Response>
where
    F: std::future::Future<Output = AppResult<S>> + Send + 'static,
    S: futures_util::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
{
    let (commit_tx, commit_rx) = tokio::sync::oneshot::channel();
    let downstream_gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lifetime = DownstreamLifetime(downstream_gone.clone());
    let commit = StreamCommit {
        sender: Arc::new(std::sync::Mutex::new(Some(commit_tx))),
        downstream_gone,
    };
    let forwarding = forwarding(commit.clone());
    let (tx, rx) = mpsc::channel::<Event>(64);
    tokio::spawn(async move {
        match forwarding.await {
            Ok(stream) => {
                commit.accept(Vec::new());
                tokio::pin!(stream);
                while let Some(Ok(event)) = stream.next().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
            Err(err) => {
                let Some(err) = commit.reject(err) else {
                    return;
                };
                let err_stream = precommitted_error_stream(downstream, err, request_id);
                tokio::pin!(err_stream);
                while let Some(Ok(event)) = err_stream.next().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let forwarded_headers = commit_rx.await.map_err(|_| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "task_panic",
            "stream forwarding task ended before commit",
        )
        .with_type("server_error")
    })??;
    let body = receiver_event_stream(rx).map(move |event| {
        let _lifetime = &lifetime;
        event
    });
    let mut response = Sse::new(body.boxed())
        .keep_alive(keep_alive)
        .into_response();
    crate::error::append_forwarded_headers(&mut response, forwarded_headers);
    Ok(response)
}

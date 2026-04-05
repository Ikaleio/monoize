use crate::error::{AppError, AppResult};
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, record_stream_done_sentinel, record_stream_terminal_event,
};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest as HandlerUrpRequest};
use crate::urp::{
    FinishReason, Item, ItemHeader, Part, PartDelta, PartHeader, Role, UrpStreamEvent,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_replicate_to_urp_events(
    urp: &HandlerUrpRequest,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4());
    let mut output_text = String::new();
    let mut started_response = false;
    let mut started_text_part = false;
    let mut finish_reason: Option<FinishReason> = None;
    let mut had_error = false;

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

        match ev.event.as_str() {
            "output" => {
                if !started_response {
                    let _ = tx
                        .send(UrpStreamEvent::ResponseStart {
                            id: response_id.clone(),
                            model: urp.model.clone(),
                            extra_body: HashMap::new(),
                        })
                        .await;
                    let _ = tx
                        .send(UrpStreamEvent::ItemStart {
                            item_index: 0,
                            header: ItemHeader::Message {
                                role: Role::Assistant,
                            },
                            extra_body: HashMap::new(),
                        })
                        .await;
                    started_response = true;
                }
                if !started_text_part {
                    let _ = tx
                        .send(UrpStreamEvent::PartStart {
                            part_index: 0,
                            item_index: 0,
                            header: PartHeader::Text,
                            extra_body: HashMap::new(),
                        })
                        .await;
                    started_text_part = true;
                }

                output_text.push_str(&ev.data);
                let _ = tx
                    .send(UrpStreamEvent::Delta {
                        part_index: 0,
                        delta: PartDelta::Text {
                            content: ev.data.clone(),
                        },
                        usage: None,
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
            "error" => {
                had_error = true;
                let _ = tx
                    .send(UrpStreamEvent::Error {
                        code: Some("replicate_error".to_string()),
                        message: ev.data.clone(),
                        extra_body: HashMap::new(),
                    })
                    .await;
            }
            "done" => {
                record_stream_done_sentinel(&runtime_metrics).await;
                if had_error {
                    finish_reason = Some(FinishReason::Other);
                } else {
                    finish_reason = Some(FinishReason::Stop);
                }
                break;
            }
            _ => {}
        }
    }

    if started_text_part {
        let _ = tx
            .send(UrpStreamEvent::PartDone {
                part_index: 0,
                part: Part::Text {
                    content: output_text.clone(),
                    extra_body: HashMap::new(),
                },
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
    }

    let output_item = Item::Message {
        role: Role::Assistant,
        parts: if output_text.is_empty() {
            Vec::new()
        } else {
            vec![Part::Text {
                content: output_text,
                extra_body: HashMap::new(),
            }]
        },
        extra_body: HashMap::new(),
    };
    let outputs = vec![output_item.clone()];

    if started_response {
        let _ = tx
            .send(UrpStreamEvent::ItemDone {
                item_index: 0,
                item: output_item,
                usage: None,
                extra_body: HashMap::new(),
            })
            .await;
        let _ = tx
            .send(UrpStreamEvent::ResponseDone {
                finish_reason,
                usage: None,
                outputs,
                extra_body: HashMap::new(),
            })
            .await;
    }

    record_stream_terminal_event(&runtime_metrics, "response.completed", None).await;
    Ok(())
}

use crate::error::AppResult;
use crate::handlers::routing::{now_ts, wrap_responses_event as _};
use crate::urp::{self};
use crate::urp::stream_helpers::*;
use axum::response::sse::Event;
use serde_json::{json, Value as _};
use tokio::sync::mpsc;

pub(crate) async fn emit_synthetic_responses_stream(
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    let mut seq = 1u64;
    let encoded = urp::encode::openai_responses::encode_response(resp, logical_model);
    let encoded_output = encoded
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let response_id = encoded
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp")
        .to_string();
    let created = encoded
        .get("created")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(now_ts);
    let base_response = json!({
        "id": response_id,
        "object": "response",
        "created": created,
        "model": logical_model,
        "status": "in_progress",
        "output": []
    });
    send_responses_event(&tx, &mut seq, "response.created", base_response.clone()).await?;
    send_responses_event(&tx, &mut seq, "response.in_progress", base_response).await?;

    for (output_index, item) in encoded_output.iter().enumerate() {
        let item_payload = json!({
            "output_index": output_index,
            "item": item.clone()
        });
        send_responses_event(&tx, &mut seq, "response.output_item.added", item_payload).await?;

        match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "reasoning" => {
                let (text, sig) = extract_reasoning_text_and_signature(item);
                if !text.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_text.delta",
                        json!({}),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
                if !sig.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.reasoning_signature.delta",
                        json!({}),
                        "delta",
                        &sig,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            "function_call" => {
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                if !arguments.is_empty() {
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.function_call_arguments.delta",
                        json!({
                            "output_index": output_index,
                            "call_id": call_id,
                            "name": name
                        }),
                        "delta",
                        arguments,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            "message" => {
                let text = extract_responses_message_text(item);
                if !text.is_empty() {
                    let phase = extract_responses_message_phase(item);
                    send_responses_delta_string(
                        &tx,
                        &mut seq,
                        "response.output_text.delta",
                        responses_text_delta_payload("", phase.as_deref()),
                        "delta",
                        &text,
                        sse_max_frame_length,
                    )
                    .await?;
                }
            }
            _ => {}
        }

        let done_item = sanitize_responses_output_item_for_frame_limit(item, sse_max_frame_length);
        send_responses_event(
            &tx,
            &mut seq,
            "response.output_item.done",
            json!({
                "output_index": output_index,
                "item": done_item
            }),
        )
        .await?;
    }
    send_responses_event(&tx, &mut seq, "response.output_text.done", json!({})).await?;
    let completed_response = sanitize_responses_completed_for_frame_limit(&encoded, sse_max_frame_length);
    send_responses_event(
        &tx,
        &mut seq,
        "response.completed",
        json!({ "response": completed_response }),
    )
    .await?;
    Ok(())
}

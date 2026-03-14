pub mod anthropic;
pub mod openai_chat;
pub mod openai_responses;

use crate::error::AppResult;
use crate::handlers::DownstreamProtocol;
use crate::urp::{self};
use axum::response::sse::Event;
use tokio::sync::mpsc;

pub(crate) async fn emit_synthetic_stream_from_urp_response(
    downstream: DownstreamProtocol,
    logical_model: &str,
    resp: &urp::UrpResponse,
    sse_max_frame_length: Option<usize>,
    tx: mpsc::Sender<Event>,
) -> AppResult<()> {
    match downstream {
        DownstreamProtocol::Responses => {
            openai_responses::emit_synthetic_responses_stream(
                logical_model,
                resp,
                sse_max_frame_length,
                tx,
            )
            .await
        }
        DownstreamProtocol::ChatCompletions => {
            openai_chat::emit_synthetic_chat_stream(
                logical_model,
                resp,
                sse_max_frame_length,
                tx,
            )
            .await
        }
        DownstreamProtocol::AnthropicMessages => {
            anthropic::emit_synthetic_messages_stream(
                logical_model,
                resp,
                sse_max_frame_length,
                tx,
            )
            .await
        }
    }
}

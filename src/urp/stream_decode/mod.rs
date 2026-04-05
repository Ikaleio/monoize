pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use crate::config::ProviderType;
use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest};
use crate::urp::UrpStreamEvent;
use axum::http::StatusCode;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub(crate) async fn stream_upstream_to_urp_events(
    urp: &UrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<UrpStreamEvent>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    match provider_type {
        ProviderType::Responses => {
            openai_responses::stream_responses_to_urp_events(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::ChatCompletion => {
            openai_chat::stream_chat_to_urp_events(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Messages => {
            anthropic::stream_messages_to_urp_events(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Gemini => {
            gemini::stream_gemini_to_urp_events(urp, upstream_resp, tx, started_at, runtime_metrics)
                .await
        }
        ProviderType::OpenaiImage => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            "openai_image does not support streaming",
        )),
        ProviderType::Replicate => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            "replicate uses dedicated handler, not URP streaming",
        )),
        ProviderType::Group => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            "group is virtual",
        )),
    }
}

pub mod anthropic;
pub mod openai_chat;
pub mod openai_responses;

use crate::config::ProviderType;
use crate::error::{AppError, AppResult};
use crate::handlers::{StreamRuntimeMetrics, UrpRequest};
use crate::urp::streaming_shared;
use axum::http::StatusCode;
use axum::response::sse::Event;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub(crate) async fn stream_upstream_sse_as_responses(
    urp: &UrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    match provider_type {
        ProviderType::Responses | ProviderType::Grok => {
            streaming_shared::stream_responses_sse_as_responses(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::ChatCompletion => {
            streaming_shared::stream_chat_sse_as_responses(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Messages => {
            streaming_shared::stream_messages_sse_as_responses(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Gemini => {
            streaming_shared::stream_gemini_sse_as_responses(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Group => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            "group is virtual",
        )),
    }
}

pub(crate) async fn stream_upstream_sse_as_chat(
    urp: &UrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    match provider_type {
        ProviderType::Gemini => {
            streaming_shared::stream_gemini_sse_as_chat(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Responses | ProviderType::Grok | ProviderType::ChatCompletion | ProviderType::Messages => {
            streaming_shared::stream_any_sse_as_chat(
                urp,
                provider_type,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Group => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            "group is virtual",
        )),
    }
}

pub(crate) async fn stream_upstream_sse_as_messages(
    urp: &UrpRequest,
    provider_type: ProviderType,
    upstream_resp: reqwest::Response,
    tx: mpsc::Sender<Event>,
    started_at: Option<std::time::Instant>,
    runtime_metrics: Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> AppResult<()> {
    match provider_type {
        ProviderType::Gemini => {
            streaming_shared::stream_gemini_sse_as_messages(
                urp,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Responses | ProviderType::Grok | ProviderType::ChatCompletion | ProviderType::Messages => {
            streaming_shared::stream_any_sse_as_messages(
                urp,
                provider_type,
                upstream_resp,
                tx,
                started_at,
                runtime_metrics,
            )
            .await
        }
        ProviderType::Group => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "provider_type_not_supported",
            "group is virtual",
        )),
    }
}

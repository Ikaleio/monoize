use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct AppError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub error_type: String,
    pub param: Option<String>,
    pub upstream_status: Option<u16>,
    pub upstream_code: Option<String>,
    pub upstream_type: Option<String>,
    pub upstream_param: Option<String>,
    /// When set, request logs use this instead of `message` so the client
    /// receives sanitized text while internal logs retain full detail.
    pub internal_message: Option<String>,
    /// The downstream encoder already sent its complete terminal error sequence.
    pub downstream_stream_terminal_sent: bool,
}

impl AppError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            error_type: "invalid_request_error".to_string(),
            param: None,
            upstream_status: None,
            upstream_code: None,
            upstream_type: None,
            upstream_param: None,
            internal_message: None,
            downstream_stream_terminal_sent: false,
        }
    }

    pub fn with_internal_message(mut self, msg: impl Into<String>) -> Self {
        self.internal_message = Some(msg.into());
        self
    }

    pub fn with_downstream_stream_terminal_sent(mut self, sent: bool) -> Self {
        self.downstream_stream_terminal_sent = sent;
        self
    }
    pub fn with_type(mut self, error_type: impl Into<String>) -> Self {
        self.error_type = error_type.into();
        self
    }

    pub fn with_param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }

    pub fn with_upstream_error(
        mut self,
        status: Option<StatusCode>,
        code: Option<String>,
        error_type: Option<String>,
        param: Option<String>,
    ) -> Self {
        self.upstream_status = status.map(|status| status.as_u16());
        self.upstream_code = code;
        self.upstream_type = error_type;
        self.upstream_param = param;
        self
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    param: Option<String>,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_param: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ErrorEnvelope {
            error: ErrorBody {
                message: self.message,
                error_type: self.error_type,
                param: self.param,
                code: self.code,
                upstream_status: self.upstream_status,
                upstream_code: self.upstream_code,
                upstream_type: self.upstream_type,
                upstream_param: self.upstream_param,
            },
        };
        (self.status, axum::Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// SAN-D4 `CHAIN`: `error` followed by each distinct `source()` message,
/// joined by `": "`. reqwest and hyper omit the cause from `Display`, so the
/// actionable detail (reset, timeout, TLS failure) lives only in the chain.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// SAN-D4 text for an SSE stream error. `EventStreamError` exposes no
/// `source()`, so the transport variant is unwrapped explicitly.
pub fn sse_error_chain(error: &eventsource_stream::EventStreamError<reqwest::Error>) -> String {
    match error {
        eventsource_stream::EventStreamError::Transport(error) => {
            format!("Transport error: {}", error_chain(error))
        }
        other => other.to_string(),
    }
}

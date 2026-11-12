use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::ops::{Deref, DerefMut};

/// SE1d `FWD`: upstream retry and rate-limit response headers.
pub type ForwardedHeaders = Vec<(HeaderName, HeaderValue)>;

/// Copies `FWD` headers onto a downstream response.
pub fn append_forwarded_headers(response: &mut Response, headers: ForwardedHeaders) {
    for (name, value) in headers {
        response.headers_mut().append(name, value);
    }
}

#[derive(Clone)]
pub struct AppError(Box<AppErrorBody>);

#[derive(Clone)]
pub struct AppErrorBody {
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
    /// SE1d: headers copied onto the downstream error response.
    pub forwarded_headers: ForwardedHeaders,
    /// RTA-8b: original status and code of a masked Channel credential failure.
    pub credential_failure: Option<CredentialFailure>,
}

impl std::fmt::Debug for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppError")
            .field("status", &self.status)
            .field("code", &self.code)
            .field("message", &self.message)
            .field("error_type", &self.error_type)
            .field("param", &self.param)
            .field("upstream_status", &self.upstream_status)
            .field("upstream_code", &self.upstream_code)
            .field("upstream_type", &self.upstream_type)
            .field("upstream_param", &self.upstream_param)
            .field("internal_message", &self.internal_message)
            .field(
                "downstream_stream_terminal_sent",
                &self.downstream_stream_terminal_sent,
            )
            .field("forwarded_headers", &self.forwarded_headers)
            .field("credential_failure", &self.credential_failure)
            .finish()
    }
}

impl Deref for AppError {
    type Target = AppErrorBody;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AppError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// RTA-8b body diagnostics. Kept apart from `upstream_status` and
/// `upstream_code` because those drive the client-visible status and code on
/// every rendering path, including WebSocket error frames.
#[derive(Debug, Clone)]
pub struct CredentialFailure {
    pub upstream_status: u16,
    pub upstream_code: Option<String>,
}

impl CredentialFailure {
    /// Adds the diagnostics to a rendered error object.
    pub fn insert_into(&self, error: &mut serde_json::Value) {
        let Some(error) = error.as_object_mut() else {
            return;
        };
        error.insert("upstream_status".to_string(), self.upstream_status.into());
        if let Some(code) = &self.upstream_code {
            error.insert("upstream_code".to_string(), code.as_str().into());
        }
    }
}

impl AppError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self(Box::new(AppErrorBody {
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
            forwarded_headers: Vec::new(),
            credential_failure: None,
        }))
    }

    pub(crate) fn into_inner(self) -> AppErrorBody {
        *self.0
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
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let error = self.into_inner();
        let (upstream_status, upstream_code) = match error.credential_failure {
            Some(failure) => (Some(failure.upstream_status), failure.upstream_code),
            None => (None, None),
        };
        let body = ErrorEnvelope {
            error: ErrorBody {
                message: error.message,
                error_type: error.error_type,
                param: error.param,
                code: error.code,
                upstream_status,
                upstream_code,
            },
        };
        let mut response = (error.status, axum::Json(body)).into_response();
        append_forwarded_headers(&mut response, error.forwarded_headers);
        response
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

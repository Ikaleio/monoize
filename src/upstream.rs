use crate::config::{ProviderAuthConfig, ProviderAuthType, ProviderConfig};
use crate::error::AppError;
use axum::http::StatusCode;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum UpstreamErrorKind {
    Network,
    Http,
}

/// SAN-D3 (`spec/upstream-error-sanitization.spec.md`): classifies where the
/// error `message` text came from, which decides how much of it may be shown
/// to downstream clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrorSource {
    /// The request could not be sent or its response body could not be read.
    Transport,
    /// Non-2xx upstream response with a parseable `error.message`.
    StructuredBody,
    /// Non-2xx upstream response whose non-empty body had no parseable
    /// `error.message`; `message` carries the raw body for server logs only.
    UnparsedBody,
    /// Non-2xx upstream response with an empty body.
    EmptyBody,
    /// Monoize-generated diagnostic (config, encoding, 2xx decode failures).
    Internal,
}

#[derive(Debug, Clone)]
pub struct UpstreamCallError {
    pub kind: UpstreamErrorKind,
    pub status: Option<StatusCode>,
    pub code: Option<String>,
    pub error_type: Option<String>,
    pub param: Option<String>,
    pub message: String,
    pub source: UpstreamErrorSource,
}

impl UpstreamCallError {
    pub fn new(kind: UpstreamErrorKind, status: Option<StatusCode>, message: String) -> Self {
        // SAN-D3 defaults: network-kind messages are transport diagnostics
        // (reqwest text may embed the upstream URL); HTTP-kind messages built
        // by constructors other than the non-2xx response path are
        // Monoize-generated diagnostics.
        let source = match kind {
            UpstreamErrorKind::Network => UpstreamErrorSource::Transport,
            UpstreamErrorKind::Http => UpstreamErrorSource::Internal,
        };
        Self {
            kind,
            status,
            code: None,
            error_type: None,
            param: None,
            message,
            source,
        }
    }

    pub fn with_error_info(mut self, info: UpstreamErrorInfo) -> Self {
        self.code = info.code;
        self.error_type = info.error_type;
        self.param = info.param;
        self
    }

    pub fn with_source(mut self, source: UpstreamErrorSource) -> Self {
        self.source = source;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpstreamErrorInfo {
    pub code: Option<String>,
    pub error_type: Option<String>,
    pub param: Option<String>,
    pub message: Option<String>,
}

pub async fn call_upstream(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: &Value,
) -> Result<Value, UpstreamCallError> {
    let resp =
        call_upstream_raw_with_timeout(client, provider, auth_value, path, body, 30_000).await?;
    let status = resp.status();
    let text = resp.text().await.map_err(|err| {
        UpstreamCallError::new(UpstreamErrorKind::Network, Some(status), err.to_string())
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        UpstreamCallError::new(UpstreamErrorKind::Http, Some(status), err.to_string())
    })?;
    Ok(value)
}

pub async fn call_upstream_raw(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: &Value,
) -> Result<reqwest::Response, UpstreamCallError> {
    call_upstream_raw_with_timeout(client, provider, auth_value, path, body, 30_000).await
}

pub async fn call_upstream_with_timeout(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: &Value,
    timeout_ms: u64,
) -> Result<Value, UpstreamCallError> {
    call_upstream_with_timeout_and_headers(
        client,
        provider,
        auth_value,
        path,
        body,
        timeout_ms,
        &[],
    )
    .await
}

pub async fn call_upstream_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: &Value,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<Value, UpstreamCallError> {
    let resp = call_upstream_raw_with_timeout_and_headers(
        client,
        provider,
        auth_value,
        path,
        body,
        timeout_ms,
        extra_headers,
    )
    .await?;
    let status = resp.status();
    let text = resp.text().await.map_err(|err| {
        UpstreamCallError::new(UpstreamErrorKind::Network, Some(status), err.to_string())
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        UpstreamCallError::new(UpstreamErrorKind::Http, Some(status), err.to_string())
    })?;
    Ok(value)
}

pub async fn call_upstream_raw_with_timeout(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: &Value,
    timeout_ms: u64,
) -> Result<reqwest::Response, UpstreamCallError> {
    call_upstream_raw_with_timeout_and_headers(
        client,
        provider,
        auth_value,
        path,
        body,
        timeout_ms,
        &[],
    )
    .await
}

pub async fn call_upstream_raw_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: &Value,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<reqwest::Response, UpstreamCallError> {
    let base = provider.base_url.as_ref().ok_or_else(|| {
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            None,
            "missing base_url".to_string(),
        )
    })?;
    let url = join_url(base, path);
    let mut req = client
        .post(url)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .json(body);
    let auth = provider.auth.as_ref().ok_or_else(|| {
        UpstreamCallError::new(UpstreamErrorKind::Http, None, "missing auth".to_string())
    })?;
    req = apply_auth(req, auth, auth_value)
        .map_err(|err| UpstreamCallError::new(UpstreamErrorKind::Http, None, err.message))?;
    for (k, v) in extra_headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|err| UpstreamCallError::new(UpstreamErrorKind::Network, None, err.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(non_success_upstream_error(resp, status).await);
    }
    Ok(resp)
}

/// SAN-D3: classify a non-2xx upstream response. `message` keeps the raw body
/// only when no structured `error.message` exists, so that the routing layer
/// can log it server-side without ever exposing it downstream.
async fn non_success_upstream_error(
    resp: reqwest::Response,
    status: StatusCode,
) -> UpstreamCallError {
    let text = resp.text().await.unwrap_or_default();
    let info = extract_error_info(&text);
    let (message, source) = match info.message.clone() {
        Some(message) => (message, UpstreamErrorSource::StructuredBody),
        None if text.is_empty() => (
            "upstream returned an empty error body".to_string(),
            UpstreamErrorSource::EmptyBody,
        ),
        None => (text, UpstreamErrorSource::UnparsedBody),
    };
    UpstreamCallError::new(UpstreamErrorKind::Http, Some(status), message)
        .with_error_info(info)
        .with_source(source)
}

pub async fn call_upstream_multipart_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    form: reqwest::multipart::Form,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<reqwest::Response, UpstreamCallError> {
    let base = provider.base_url.as_ref().ok_or_else(|| {
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            None,
            "missing base_url".to_string(),
        )
    })?;
    let url = join_url(base, path);
    let mut req = client
        .post(url)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .multipart(form);
    let auth = provider.auth.as_ref().ok_or_else(|| {
        UpstreamCallError::new(UpstreamErrorKind::Http, None, "missing auth".to_string())
    })?;
    req = apply_auth(req, auth, auth_value)
        .map_err(|err| UpstreamCallError::new(UpstreamErrorKind::Http, None, err.message))?;
    for (k, v) in extra_headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|err| UpstreamCallError::new(UpstreamErrorKind::Network, None, err.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(non_success_upstream_error(resp, status).await);
    }
    Ok(resp)
}

pub async fn call_responses(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    body: &Value,
) -> Result<Value, UpstreamCallError> {
    call_upstream(client, provider, auth_value, "/v1/responses", body).await
}

pub async fn call_chat_completions(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    body: &Value,
) -> Result<Value, UpstreamCallError> {
    call_upstream(client, provider, auth_value, "/v1/chat/completions", body).await
}

pub async fn call_messages(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    body: &Value,
) -> Result<Value, UpstreamCallError> {
    call_upstream(client, provider, auth_value, "/v1/messages", body).await
}

#[allow(clippy::result_large_err)]
fn apply_auth(
    req: reqwest::RequestBuilder,
    auth: &ProviderAuthConfig,
    auth_value: &str,
) -> Result<reqwest::RequestBuilder, AppError> {
    match auth.auth_type {
        ProviderAuthType::Bearer => Ok(req.bearer_auth(auth_value)),
        ProviderAuthType::Header => {
            let header_name = auth
                .header_name
                .clone()
                .unwrap_or_else(|| "x-api-key".to_string());
            Ok(req.header(header_name, auth_value))
        }
        ProviderAuthType::Query => {
            let query_name = auth
                .query_name
                .clone()
                .unwrap_or_else(|| "api_key".to_string());
            Ok(req.query(&[(query_name, auth_value)]))
        }
    }
}

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let mut path = path.trim_start_matches('/');
    if base.ends_with("/v1") {
        if path == "v1" {
            path = "";
        } else if let Some(stripped) = path.strip_prefix("v1/") {
            path = stripped;
        }
    }
    if path.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{path}")
    }
}

fn extract_error_info(text: &str) -> UpstreamErrorInfo {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return UpstreamErrorInfo::default();
    };
    let Some(error) = value.get("error") else {
        return UpstreamErrorInfo::default();
    };
    let metadata = error.get("metadata").and_then(Value::as_object);
    UpstreamErrorInfo {
        code: error.get("code").and_then(json_scalar_string).or_else(|| {
            metadata
                .and_then(|metadata| metadata.get("provider_code"))
                .and_then(json_scalar_string)
        }),
        error_type: error.get("type").and_then(json_scalar_string).or_else(|| {
            metadata
                .and_then(|metadata| metadata.get("error_type"))
                .and_then(json_scalar_string)
        }),
        param: error
            .get("param")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        message: error
            .get("message")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

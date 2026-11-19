use crate::config::{ProviderAuthConfig, ProviderAuthType, ProviderConfig};
use crate::error::{AppError, ForwardedHeaders, error_chain};
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
    pub(crate) reported_error: UpstreamErrorFields,
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
            reported_error: UpstreamErrorFields::default(),
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

    pub(crate) fn failure_response(&self) -> FailureResponse {
        self.reported_error
            .0
            .as_ref()
            .and_then(|reported| reported.response)
            .unwrap_or_else(|| match self.source {
                UpstreamErrorSource::EmptyBody
                | UpstreamErrorSource::UnparsedBody
                | UpstreamErrorSource::StructuredBody => self
                    .status
                    .map(|status| FailureResponse::new(status, None))
                    .unwrap_or_default(),
                _ => FailureResponse::default(),
            })
    }

    pub(crate) fn take_forwarded_headers(&mut self) -> ForwardedHeaders {
        self.reported_error
            .0
            .as_mut()
            .map(|metadata| std::mem::take(&mut metadata.forwarded_headers))
            .unwrap_or_default()
    }

    fn set_forwarded_headers(&mut self, forwarded_headers: ForwardedHeaders) {
        if !forwarded_headers.is_empty() {
            self.reported_error
                .0
                .get_or_insert_with(Box::default)
                .forwarded_headers = forwarded_headers;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpstreamErrorInfo {
    pub code: Option<String>,
    pub error_type: Option<String>,
    pub param: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UpstreamErrorFields(Option<Box<ReportedFailure>>);

#[derive(Debug, Clone, Default)]
// Keep infrequent response detail together so other errors fit inline without allocations.
struct ReportedFailure {
    fields: [Option<String>; 3],
    reported: bool,
    response: Option<FailureResponse>,
    forwarded_headers: ForwardedHeaders,
}

impl UpstreamErrorFields {
    pub(crate) fn from_json(error: &Value) -> Self {
        Self::from_members(|key| error.get(key).and_then(Value::as_str))
    }

    pub(crate) fn from_members<'a>(member: impl Fn(&str) -> Option<&'a str>) -> Self {
        Self(Some(Box::new(ReportedFailure {
            fields: ["message", "code", "type"].map(|key| member(key).map(str::to_owned)),
            reported: true,
            ..Default::default()
        })))
    }

    pub(crate) fn attach_failure_response(&mut self, response: FailureResponse) {
        self.0.get_or_insert_with(Box::default).response = Some(response);
    }

    pub(crate) fn retry_after_seconds(&self) -> Option<u16> {
        self.0.as_ref()?.response?.retry_after_seconds()
    }

    pub(crate) fn is_reported(&self) -> bool {
        self.0.as_ref().is_some_and(|metadata| metadata.reported)
    }

    pub(crate) fn classify(
        &self,
        rules: &crate::settings::CompiledFailureRules,
        http_status: Option<u16>,
    ) -> Option<crate::settings::RetryableFailureClass> {
        let [message, code, kind] = &self.0.as_ref().filter(|metadata| metadata.reported)?.fields;
        rules.classify(
            http_status,
            [message.as_deref(), code.as_deref(), kind.as_deref()],
        )
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FailureResponse {
    status: Option<StatusCode>,
    retry_after_seconds: Option<u16>,
}

impl FailureResponse {
    pub(crate) fn from_http(response: &reqwest::Response) -> Self {
        Self::new(
            response.status(),
            parse_retry_after(response.headers(), chrono::Utc::now().timestamp()),
        )
    }

    fn new(status: StatusCode, retry_after_seconds: Option<u16>) -> Self {
        Self {
            status: Some(status),
            retry_after_seconds,
        }
    }

    pub(crate) fn status(self) -> Option<u16> {
        self.status.map(|status| status.as_u16())
    }

    pub(crate) fn retry_after_seconds(self) -> Option<u16> {
        self.retry_after_seconds
    }

    pub(crate) fn apply(self, error: &mut AppError) {
        error.actual_upstream_status = self.status();
        error.retry_after_seconds = self.retry_after_seconds();
    }
}

/// Byte length of `value` serialized as compact JSON, computed without
/// allocating the serialized text.
pub fn serialized_json_len(value: &Value) -> usize {
    struct ByteCounter(usize);
    impl std::io::Write for ByteCounter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value)
        .expect("serializing a JSON value into an infallible writer cannot fail");
    counter.0
}

/// An upstream JSON request body serialized once into an exactly sized
/// buffer (FP6j-2). The bytes equal `serde_json::to_vec` of the source value,
/// so the caller can drop that value before awaiting the upstream response.
pub struct JsonBody {
    bytes: Vec<u8>,
    large: Option<Value>,
}

impl JsonBody {
    pub fn new(value: &Value) -> Self {
        if crate::payload::contains_large(value) {
            return Self {
                bytes: Vec::new(),
                large: Some(value.clone()),
            };
        }
        let mut bytes = Vec::with_capacity(serialized_json_len(value));
        serde_json::to_writer(&mut bytes, value)
            .expect("serializing a JSON value into memory cannot fail");
        Self { bytes, large: None }
    }

    /// UTF-8 text of the body, for transports that send text frames.
    pub fn into_string(self) -> String {
        String::from_utf8(self.bytes).expect("serde_json always writes UTF-8")
    }
}

/// SE1d `FWD`: selects the retry and rate-limit headers of an upstream response.
pub fn forwarded_response_headers(headers: &reqwest::header::HeaderMap) -> ForwardedHeaders {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            matches!(name, "retry-after" | "retry-after-ms" | "x-should-retry")
                || name.starts_with("anthropic-ratelimit-")
                || name.starts_with("x-ratelimit-")
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Sends a JSON request and reads the whole 2xx body as JSON. CFG-6a item 1:
/// `timeout_ms` bounds sending, the response headers, and the full body.
pub async fn call_upstream_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: JsonBody,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<Value, UpstreamCallError> {
    call_upstream_json_with_response_headers(
        client,
        provider,
        auth_value,
        path,
        body,
        timeout_ms,
        extra_headers,
    )
    .await
    .map(|(value, _, _)| value)
}

/// `call_upstream_with_timeout_and_headers` that also returns SE1d `FWD`.
pub(crate) async fn call_upstream_json_with_response_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: JsonBody,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<(Value, ForwardedHeaders, FailureResponse), UpstreamCallError> {
    let req = json_request(client, provider, auth_value, path, body, extra_headers)?;
    with_deadline(timeout_ms, "upstream response not completed", async {
        let resp = exchange(req).await?;
        let forwarded = forwarded_response_headers(resp.headers());
        let failure_response = FailureResponse::from_http(&resp);
        match read_json_body(resp).await {
            Ok(value) => Ok((value, forwarded, failure_response)),
            Err(mut err) => {
                err.set_forwarded_headers(forwarded);
                Err(err)
            }
        }
    })
    .await
}

/// Sends a JSON request and returns the 2xx response with its body unread.
/// CFG-6a item 2: `timeout_ms` bounds only the response headers (and a
/// non-2xx error body); the caller bounds the 2xx body by idle time.
pub async fn call_upstream_raw_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: JsonBody,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<reqwest::Response, UpstreamCallError> {
    let req = json_request(client, provider, auth_value, path, body, extra_headers)?;
    with_deadline(
        timeout_ms,
        "upstream response headers not received",
        exchange(req),
    )
    .await
}

/// Multipart counterpart of `call_upstream_with_timeout_and_headers`.
pub async fn call_upstream_multipart_json_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    form: reqwest::multipart::Form,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<Value, UpstreamCallError> {
    let req = multipart_request(client, provider, auth_value, path, form, extra_headers)?;
    with_deadline(timeout_ms, "upstream response not completed", async {
        read_json_body(exchange(req).await?).await
    })
    .await
}

/// Multipart counterpart of `call_upstream_raw_with_timeout_and_headers`.
pub async fn call_upstream_multipart_with_timeout_and_headers(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    form: reqwest::multipart::Form,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
) -> Result<reqwest::Response, UpstreamCallError> {
    let req = multipart_request(client, provider, auth_value, path, form, extra_headers)?;
    with_deadline(
        timeout_ms,
        "upstream response headers not received",
        exchange(req),
    )
    .await
}

pub(crate) fn json_request(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    body: JsonBody,
    extra_headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder, UpstreamCallError> {
    // Same header and bytes that `RequestBuilder::json` sets on a fresh request.
    let req = client
        .post(upstream_url(provider, path)?)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    let req = match body.large {
        Some(value) => req.body(reqwest::Body::wrap_stream(crate::payload::json_stream(
            value,
        ))),
        None => req.body(body.bytes),
    };
    authorize(req, provider, auth_value, extra_headers)
}

fn multipart_request(
    client: &reqwest::Client,
    provider: &ProviderConfig,
    auth_value: &str,
    path: &str,
    form: reqwest::multipart::Form,
    extra_headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder, UpstreamCallError> {
    let req = client.post(upstream_url(provider, path)?).multipart(form);
    authorize(req, provider, auth_value, extra_headers)
}

fn upstream_url(provider: &ProviderConfig, path: &str) -> Result<String, UpstreamCallError> {
    let base = provider.base_url.as_ref().ok_or_else(|| {
        UpstreamCallError::new(
            UpstreamErrorKind::Http,
            None,
            "missing base_url".to_string(),
        )
    })?;
    Ok(join_url(base, path))
}

fn authorize(
    req: reqwest::RequestBuilder,
    provider: &ProviderConfig,
    auth_value: &str,
    extra_headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder, UpstreamCallError> {
    let auth = provider.auth.as_ref().ok_or_else(|| {
        UpstreamCallError::new(UpstreamErrorKind::Http, None, "missing auth".to_string())
    })?;
    let mut req = apply_auth(req, auth, auth_value).map_err(|err| {
        UpstreamCallError::new(UpstreamErrorKind::Http, None, err.into_inner().message)
    })?;
    for (k, v) in extra_headers {
        req = req.header(k, v);
    }
    Ok(req)
}

/// Sends `req` and returns the response when its status is 2xx.
async fn exchange(req: reqwest::RequestBuilder) -> Result<reqwest::Response, UpstreamCallError> {
    let resp = send_request(req).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(non_success_upstream_error(resp, status).await);
    }
    Ok(resp)
}

pub(crate) async fn send_request(
    req: reqwest::RequestBuilder,
) -> Result<reqwest::Response, UpstreamCallError> {
    req.send().await.map_err(|err| transport_error(None, &err))
}

pub(crate) async fn read_json_body(resp: reqwest::Response) -> Result<Value, UpstreamCallError> {
    let status = resp.status();
    if let Some(payload) = crate::payload::current() {
        use futures_util::TryStreamExt as _;
        let stream = resp
            .bytes_stream()
            .map_err(|error| crate::payload::ReadError::Transport(error_chain(&error)));
        return crate::payload::read_json(stream, payload, crate::payload::WIRE_LIMIT)
            .await
            .map_err(|error| {
                let kind = match error {
                    crate::payload::ReadError::Transport(_) => UpstreamErrorKind::Network,
                    _ => UpstreamErrorKind::Http,
                };
                UpstreamCallError::new(kind, Some(status), error.to_string())
            });
    }
    let bytes = crate::bounded_response::read_response_body_with_limit(resp, 50 * 1024 * 1024)
        .await
        .map_err(|error| {
            UpstreamCallError::new(UpstreamErrorKind::Network, Some(status), error.to_string())
        })?;
    serde_json::from_slice(&bytes).map_err(|err| {
        UpstreamCallError::new(UpstreamErrorKind::Http, Some(status), err.to_string())
    })
}

fn transport_error(status: Option<StatusCode>, err: &reqwest::Error) -> UpstreamCallError {
    UpstreamCallError::new(UpstreamErrorKind::Network, status, error_chain(err))
}

/// CFG-6a item 4: an expired deadline is a transport failure.
async fn with_deadline<T>(
    timeout_ms: u64,
    what: &str,
    call: impl Future<Output = Result<T, UpstreamCallError>>,
) -> Result<T, UpstreamCallError> {
    tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), call)
        .await
        .unwrap_or_else(|_| {
            Err(UpstreamCallError::new(
                UpstreamErrorKind::Network,
                None,
                format!("{what} within {timeout_ms}ms"),
            ))
        })
}

/// SAN-D3: classify a non-2xx upstream response. `message` keeps the raw body
/// only when no structured `error.message` exists, so that the routing layer
/// can log it server-side without ever exposing it downstream.
async fn non_success_upstream_error(
    resp: reqwest::Response,
    status: StatusCode,
) -> UpstreamCallError {
    let forwarded_headers = forwarded_response_headers(resp.headers());
    let retry_after_seconds = parse_retry_after(resp.headers(), chrono::Utc::now().timestamp());
    let (text, _) = read_http_error_body(resp).await;
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let mut info = parsed
        .as_ref()
        .map_or_else(UpstreamErrorInfo::default, |value| {
            let mut info = extract_error_members(reported_error_object(value));
            if !value.get("error").is_some_and(Value::is_object) {
                info.message = None;
            }
            info
        });
    let (message, source) = match info.message.take() {
        Some(message) => (message, UpstreamErrorSource::StructuredBody),
        None if text.is_empty() => (
            "upstream returned an empty error body".to_string(),
            UpstreamErrorSource::EmptyBody,
        ),
        None => (text, UpstreamErrorSource::UnparsedBody),
    };
    let mut err = UpstreamCallError::new(UpstreamErrorKind::Http, Some(status), message)
        .with_error_info(info)
        .with_source(source);
    if let Some(value) = parsed.as_ref() {
        err.reported_error = UpstreamErrorFields::from_json(reported_error_object(value));
    }
    if err.reported_error.is_reported() || retry_after_seconds.is_some() {
        err.reported_error
            .attach_failure_response(FailureResponse::new(status, retry_after_seconds));
    }
    err.set_forwarded_headers(forwarded_headers);
    err
}

pub(crate) async fn read_http_error_body(resp: reqwest::Response) -> (String, bool) {
    let mut chunks = resp.bytes_stream();
    use futures_util::StreamExt as _;
    let mut text = Vec::new();
    const ERROR_MAX: usize = 64 * 1024;
    let mut timed_out = false;
    while let Some(result) = chunks.next().await {
        let chunk = match result {
            Ok(chunk) => chunk,
            Err(error) => {
                timed_out = error.is_timeout();
                break;
            }
        };
        let take = chunk.len().min(ERROR_MAX - text.len());
        text.extend_from_slice(&chunk[..take]);
        if text.len() == ERROR_MAX {
            break;
        }
    }
    (String::from_utf8_lossy(&text).into_owned(), timed_out)
}

pub(crate) fn parse_retry_after(headers: &axum::http::HeaderMap, now: i64) -> Option<u16> {
    const MAX_SECONDS: u64 = 3600;

    fn integer(value: &str) -> Option<u64> {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse().unwrap_or(u64::MAX))
    }

    let header = |name: &str| headers.get(name)?.to_str().ok().map(str::trim);
    if let Some(ms) = header("retry-after-ms").and_then(integer) {
        return Some(ms.div_ceil(1000).min(MAX_SECONDS) as u16);
    }
    let value = header("retry-after")?;
    if let Some(seconds) = integer(value) {
        return Some(seconds.min(MAX_SECONDS) as u16);
    }
    let date = parse_http_date(value, now)?;
    Some(
        date.and_utc()
            .timestamp()
            .saturating_sub(now)
            .clamp(0, MAX_SECONDS as i64) as u16,
    )
}

fn parse_http_date(value: &str, now: i64) -> Option<chrono::NaiveDateTime> {
    use chrono::format::{Parsed, StrftimeItems, parse};
    use chrono::{DateTime, Datelike, Months, NaiveDateTime};

    if let Some(date) = ["%a, %d %b %Y %H:%M:%S GMT", "%a %b %e %H:%M:%S %Y"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
    {
        return Some(date);
    }

    let mut parsed = Parsed::new();
    parse(
        &mut parsed,
        value,
        StrftimeItems::new("%A, %d-%b-%y %H:%M:%S GMT"),
    )
    .ok()?;
    // RFC 9110 interprets an RFC 850 year more than 50 years ahead as a past year.
    let cutoff = DateTime::from_timestamp(now, 0)?.checked_add_months(Months::new(600))?;
    let suffix = parsed.year_mod_100()?;
    let mut year = cutoff.year() - (cutoff.year() - suffix).rem_euclid(100);
    if year == cutoff.year()
        && (parsed.month()?, parsed.day()?, parsed.to_naive_time().ok()?)
            > (cutoff.month(), cutoff.day(), cutoff.time())
    {
        year -= 100;
    }
    parsed.set_year(i64::from(year)).ok()?;
    parsed.to_naive_datetime_with_offset(0).ok()
}

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
            let req = req.header(header_name.as_str(), auth_value);
            // Official Anthropic authenticates `x-api-key`. Many Messages-compatible
            // relays authenticate Bearer. Send both when the header is `x-api-key`.
            if header_name.eq_ignore_ascii_case("x-api-key") {
                Ok(req.bearer_auth(auth_value))
            } else {
                Ok(req)
            }
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

/// API version path segments recognized at the end of a base URL and at the start of a path.
const API_VERSION_SEGMENTS: [&str; 3] = ["v1", "v1beta", "v1alpha"];

pub(crate) fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let mut path = path.trim_start_matches('/');
    let base_has_version = base
        .rsplit_once('/')
        .is_some_and(|(_, last)| API_VERSION_SEGMENTS.contains(&last));
    if base_has_version {
        // The configured base version wins over the version baked into the path.
        let (first, rest) = path.split_once('/').unwrap_or((path, ""));
        if API_VERSION_SEGMENTS.contains(&first) {
            path = rest;
        }
    }
    if path.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{path}")
    }
}

pub(crate) fn reported_error_object(value: &Value) -> &Value {
    value
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(value)
}

pub(crate) fn extract_error_members(error: &Value) -> UpstreamErrorInfo {
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

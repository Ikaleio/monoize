use crate::error::AppError;
use crate::upstream::join_url;
use async_trait::async_trait;
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, mpsc, watch};
use tokio::time::{Duration, timeout};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Request};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async_tls_with_config, client_async_with_config,
    connect_async_with_config,
};

const RESPONSES_WS_BETA: &str = "responses_websockets=2026-02-06";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponsesWsControl {
    Running,
    Interrupt,
    Disconnected,
}

pub fn ensure_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[derive(Debug)]
pub struct HandshakeError {
    pub message: String,
    pub http_status: Option<u16>,
}

impl HandshakeError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            http_status: None,
        }
    }

    fn with_status(message: impl Into<String>, http_status: u16) -> Self {
        Self {
            message: message.into(),
            http_status: Some(http_status),
        }
    }
}

#[async_trait]
pub trait ResponsesWsSession: Send {
    async fn send_text(&mut self, text: String) -> Result<(), String>;
    async fn next_text(&mut self) -> Result<Option<String>, String>;
    async fn close(&mut self);
    /// `x-codex-turn-state` returned in the `101` response headers (WS2b).
    fn handshake_turn_state(&self) -> Option<String> {
        None
    }
}

trait WsIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> WsIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
type BoxedIo = Box<dyn WsIo>;

enum InnerWs {
    Direct(WebSocketStream<MaybeTlsStream<TcpStream>>),
    Plain(WebSocketStream<BoxedIo>),
    NestedTls(WebSocketStream<MaybeTlsStream<BoxedIo>>),
}

struct TungsteniteResponsesWs {
    inner: InnerWs,
    turn_state: Option<String>,
}

#[async_trait]
impl ResponsesWsSession for TungsteniteResponsesWs {
    fn handshake_turn_state(&self) -> Option<String> {
        self.turn_state.clone()
    }

    async fn send_text(&mut self, text: String) -> Result<(), String> {
        send_message(&mut self.inner, Message::Text(text.into())).await
    }

    async fn next_text(&mut self) -> Result<Option<String>, String> {
        loop {
            match recv_message(&mut self.inner).await? {
                None => return Ok(None),
                Some(Message::Text(text)) => return Ok(Some(text.to_string())),
                Some(Message::Ping(payload)) => {
                    send_message(&mut self.inner, Message::Pong(payload)).await?;
                }
                Some(Message::Pong(_)) | Some(Message::Frame(_)) => {}
                Some(Message::Close(_)) => return Ok(None),
                Some(Message::Binary(_)) => {
                    return Err("upstream WebSocket sent a binary frame".to_string());
                }
            }
        }
    }

    async fn close(&mut self) {
        let _ = match &mut self.inner {
            InnerWs::Direct(ws) => ws.close(None).await,
            InnerWs::Plain(ws) => ws.close(None).await,
            InnerWs::NestedTls(ws) => ws.close(None).await,
        };
    }
}

async fn send_message(inner: &mut InnerWs, message: Message) -> Result<(), String> {
    match inner {
        InnerWs::Direct(ws) => ws.send(message).await,
        InnerWs::Plain(ws) => ws.send(message).await,
        InnerWs::NestedTls(ws) => ws.send(message).await,
    }
    .map_err(|err| err.to_string())
}

async fn recv_message(inner: &mut InnerWs) -> Result<Option<Message>, String> {
    let next = match inner {
        InnerWs::Direct(ws) => ws.next().await,
        InnerWs::Plain(ws) => ws.next().await,
        InnerWs::NestedTls(ws) => ws.next().await,
    };
    match next {
        None => Ok(None),
        Some(Ok(message)) => Ok(Some(message)),
        Some(Err(err)) => Err(err.to_string()),
    }
}

pub fn responses_ws_create_payload(mut body: serde_json::Value) -> serde_json::Value {
    if let Some(object) = body.as_object_mut() {
        object.remove("stream");
        object.remove("background");
        object.remove("generate");
        object.remove("previous_response_id");
        object.remove("type");
        object.insert("type".into(), serde_json::json!("response.create"));
    }
    body
}

pub fn responses_authorization_header(api_key: &str) -> (String, String) {
    ("Authorization".to_string(), format!("Bearer {api_key}"))
}

pub fn responses_ws_beta_header() -> (String, String) {
    ("OpenAI-Beta".to_string(), RESPONSES_WS_BETA.to_string())
}

pub async fn connect_responses_websocket(
    base_url: &str,
    headers: &[(String, String)],
    proxy_url: Option<&str>,
    timeout_ms: u64,
) -> Result<Box<dyn ResponsesWsSession>, HandshakeError> {
    let timeout_ms = timeout_ms.max(1);
    timeout(
        Duration::from_millis(timeout_ms),
        connect_responses_websocket_inner(base_url, headers, proxy_url),
    )
    .await
    .map_err(|_| {
        HandshakeError::new(format!(
            "upstream WebSocket upgrade timed out after {timeout_ms}ms"
        ))
    })?
}

async fn connect_responses_websocket_inner(
    base_url: &str,
    headers: &[(String, String)],
    proxy_url: Option<&str>,
) -> Result<Box<dyn ResponsesWsSession>, HandshakeError> {
    ensure_rustls_crypto_provider();
    let http_url = join_url(base_url, "/v1/responses");
    let ws_url = http_url_to_ws(&http_url)?;
    let request = build_upgrade_request(&ws_url, headers)?;
    let target =
        reqwest::Url::parse(&ws_url).map_err(|err| HandshakeError::new(err.to_string()))?;
    let use_tls = target.scheme() == "wss";
    let session =
        if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
            connect_via_proxy(&target, request, proxy_url, use_tls).await?
        } else {
            let (stream, response) =
                connect_async_with_config(request, Some(upstream_ws_config()), false)
                    .await
                    .map_err(classify_connect_error)?;
            ensure_switching_protocols(response.status().as_u16())?;
            TungsteniteResponsesWs {
                inner: InnerWs::Direct(stream),
                turn_state: response_turn_state(&response),
            }
        };
    Ok(Box::new(session))
}

fn build_upgrade_request(
    ws_url: &str,
    headers: &[(String, String)],
) -> Result<Request<()>, HandshakeError> {
    let mut request = ws_url
        .into_client_request()
        .map_err(|err| HandshakeError::new(err.to_string()))?;
    request.headers_mut().insert(
        HeaderName::from_static("openai-beta"),
        HeaderValue::from_static(RESPONSES_WS_BETA),
    );
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("openai-beta") {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| HandshakeError::new(format!("invalid header name {name}: {err}")))?;
        let header_value = HeaderValue::from_str(value).map_err(|err| {
            HandshakeError::new(format!("invalid header value for {name}: {err}"))
        })?;
        request.headers_mut().insert(header_name, header_value);
    }
    Ok(request)
}

async fn connect_via_proxy(
    target: &reqwest::Url,
    request: Request<()>,
    proxy_url: &str,
    use_tls: bool,
) -> Result<TungsteniteResponsesWs, HandshakeError> {
    let proxy =
        reqwest::Url::parse(proxy_url).map_err(|err| HandshakeError::new(err.to_string()))?;
    if proxy.scheme() != "http" && proxy.scheme() != "https" {
        return Err(HandshakeError::new(format!(
            "unsupported proxy scheme {}",
            proxy.scheme()
        )));
    }
    let proxy_host = proxy
        .host_str()
        .ok_or_else(|| HandshakeError::new("proxy URL is missing a host"))?;
    let proxy_port = proxy
        .port_or_known_default()
        .ok_or_else(|| HandshakeError::new("proxy URL is missing a port"))?;
    let tcp = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|err| HandshakeError::new(err.to_string()))?;
    let proxy_auth = proxy_basic_auth(&proxy);
    let target_host = target
        .host_str()
        .ok_or_else(|| HandshakeError::new("upstream URL is missing a host"))?;
    let target_port = target
        .port_or_known_default()
        .ok_or_else(|| HandshakeError::new("upstream URL is missing a port"))?;

    let tunneled: BoxedIo = if proxy.scheme() == "https" {
        let tls = tls_connect(tcp, proxy_host).await?;
        http_connect(tls, target_host, target_port, proxy_auth.as_deref()).await?
    } else {
        http_connect(tcp, target_host, target_port, proxy_auth.as_deref()).await?
    };

    if use_tls {
        let (stream, response) =
            client_async_tls_with_config(request, tunneled, Some(upstream_ws_config()), None)
                .await
                .map_err(classify_connect_error)?;
        ensure_switching_protocols(response.status().as_u16())?;
        Ok(TungsteniteResponsesWs {
            inner: InnerWs::NestedTls(stream),
            turn_state: response_turn_state(&response),
        })
    } else {
        let (stream, response) =
            client_async_with_config(request, tunneled, Some(upstream_ws_config()))
                .await
                .map_err(classify_connect_error)?;
        ensure_switching_protocols(response.status().as_u16())?;
        Ok(TungsteniteResponsesWs {
            inner: InnerWs::Plain(stream),
            turn_state: response_turn_state(&response),
        })
    }
}

async fn http_connect<S>(
    mut stream: S,
    host: &str,
    port: u16,
    proxy_auth: Option<&str>,
) -> Result<BoxedIo, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let authority = connect_authority(host, port);
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = proxy_auth {
        request.push_str("Proxy-Authorization: Basic ");
        request.push_str(auth);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| HandshakeError::new(err.to_string()))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 256];
    loop {
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|err| HandshakeError::new(err.to_string()))?;
        if n == 0 {
            return Err(HandshakeError::new("proxy closed during CONNECT"));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            return Err(HandshakeError::new("proxy CONNECT response is too large"));
        }
    }
    let header_end = connect_header_end(&buf)?;
    let leftover = buf[header_end..].to_vec();
    if leftover.is_empty() {
        Ok(Box::new(stream))
    } else {
        Ok(Box::new(PrefixedIo {
            prefix: leftover,
            offset: 0,
            inner: stream,
        }))
    }
}

struct PrefixedIo<S> {
    prefix: Vec<u8>,
    offset: usize,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.offset < self.prefix.len() {
            let remaining = &self.prefix[self.offset..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.offset += n;
            if self.offset == self.prefix.len() {
                self.prefix.clear();
                self.offset = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn tls_connect(stream: TcpStream, host: &str) -> Result<BoxedIo, HandshakeError> {
    let connector = TlsConnector::from(rustls_client_config());
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|err| HandshakeError::new(format!("invalid TLS host {host}: {err}")))?;
    let tls = connector
        .connect(server_name, stream)
        .await
        .map_err(|err| HandshakeError::new(err.to_string()))?;
    Ok(Box::new(tls))
}

fn rustls_client_config() -> Arc<rustls::ClientConfig> {
    ensure_rustls_crypto_provider();
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            for cert in native.certs {
                let _ = roots.add(cert);
            }
            Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

fn proxy_basic_auth(proxy: &reqwest::Url) -> Option<String> {
    let username = proxy.username();
    if username.is_empty() {
        return None;
    }
    let password = proxy.password().unwrap_or_default();
    Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("{username}:{password}"),
    ))
}

fn http_url_to_ws(http_url: &str) -> Result<String, HandshakeError> {
    let url = reqwest::Url::parse(http_url).map_err(|err| HandshakeError::new(err.to_string()))?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => url.scheme(),
        other => {
            return Err(HandshakeError::new(format!(
                "unsupported upstream scheme {other}"
            )));
        }
    };
    let mut converted = url.clone();
    converted
        .set_scheme(scheme)
        .map_err(|_| HandshakeError::new("failed to convert upstream URL to WebSocket"))?;
    Ok(converted.to_string())
}

fn upstream_ws_config() -> WebSocketConfig {
    let max = std::env::var("MONOIZE_RESPONSES_WS_MESSAGE_MAX_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(50 * 1024 * 1024);
    WebSocketConfig::default()
        .max_message_size(Some(max))
        .max_frame_size(Some(max))
}

fn connect_authority(host: &str, port: u16) -> String {
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn connect_header_end(buf: &[u8]) -> Result<usize, HandshakeError> {
    let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err(HandshakeError::new(
            "proxy CONNECT response is missing a header terminator",
        ));
    };
    let header_end = pos + 4;
    let header_text = String::from_utf8_lossy(&buf[..header_end]);
    let status_line = header_text.lines().next().unwrap_or_default();
    let status = parse_http_status_line(status_line).unwrap_or(0);
    if status != 200 {
        return Err(HandshakeError::with_status(
            format!("proxy CONNECT failed: {status_line}"),
            status,
        ));
    }
    Ok(header_end)
}

fn parse_http_status_line(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

fn ensure_switching_protocols(status: u16) -> Result<(), HandshakeError> {
    if status == 101 {
        Ok(())
    } else {
        Err(HandshakeError::with_status(
            format!("upstream WebSocket upgrade returned {status}"),
            status,
        ))
    }
}

fn classify_connect_error(err: tokio_tungstenite::tungstenite::Error) -> HandshakeError {
    match err {
        tokio_tungstenite::tungstenite::Error::Http(response) => HandshakeError::with_status(
            format!(
                "upstream WebSocket upgrade returned {}",
                response.status().as_u16()
            ),
            response.status().as_u16(),
        ),
        other => HandshakeError::new(other.to_string()),
    }
}

pub fn handshake_app_error(err: HandshakeError) -> AppError {
    AppError::new(
        StatusCode::BAD_GATEWAY,
        "upstream_websocket_handshake_failed",
        err.message,
    )
}

fn response_turn_state<B>(
    response: &tokio_tungstenite::tungstenite::http::Response<B>,
) -> Option<String> {
    response
        .headers()
        .get(TURN_STATE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

const TURN_STATE_HEADER: &str = "x-codex-turn-state";
pub(crate) const UPSTREAM_KIND_WEBSOCKET: u8 = 1;
pub(crate) const UPSTREAM_KIND_HTTP: u8 = 2;

/// WS2h: one lane's pinned upstream session and its continuation basis.
#[derive(Default)]
pub(crate) struct ResponsesWsSlot {
    session: Option<Box<dyn ResponsesWsSession>>,
    channel_id: String,
    upstream_model: String,
    last_upstream_id: Option<String>,
    last_public_id: Option<String>,
    basis: Vec<Value>,
    /// `(type, id)` of each item in the last upstream terminal `output`; the
    /// incremental create must find these ids right after `basis` (WS2h item 2).
    last_output: Vec<(String, Option<String>)>,
    /// Number of input items of steers the upstream left pending after `last_output`;
    /// the upstream prepends them itself on an incremental create.
    pending_steer_parts: usize,
    turn_state: Option<String>,
    turn_state_channel: String,
    stashed: Vec<String>,
    /// Upstream response id -> public response id for every response read on
    /// this lane, so relayed acknowledgements can be rewritten (WS19).
    public_ids: HashMap<String, String>,
}

impl ResponsesWsSlot {
    fn forget_session_state(&mut self) {
        self.session = None;
        self.last_upstream_id = None;
        self.last_public_id = None;
        self.basis.clear();
        self.last_output.clear();
        self.pending_steer_parts = 0;
        self.stashed.clear();
    }
}

/// WS2i: a stashed automatic successor waiting for its WS18 turn.
pub(crate) struct ResponsesWsSuccessor {
    pub(crate) steer_inputs: Vec<Value>,
    pub(crate) channel_id: String,
    pub(crate) upstream_model: String,
}

pub(crate) struct ResponsesWsLane {
    slot: Arc<AsyncMutex<ResponsesWsSlot>>,
    relay: mpsc::UnboundedSender<Value>,
    successor: std::sync::Mutex<Option<ResponsesWsSuccessor>>,
    /// WS2h item 6: an idle watcher holds the slot between turns until woken.
    watching: AtomicBool,
    wake: tokio::sync::Notify,
}

impl ResponsesWsLane {
    /// `relay` receives acknowledgement events that bypass the stream decoder.
    pub(crate) fn new(relay: mpsc::UnboundedSender<Value>) -> Self {
        Self {
            slot: Arc::new(AsyncMutex::new(ResponsesWsSlot::default())),
            relay,
            successor: std::sync::Mutex::new(None),
            watching: AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
        }
    }

    pub(crate) fn take_successor(&self) -> Option<ResponsesWsSuccessor> {
        self.successor.lock().ok()?.take()
    }

    pub(crate) fn has_successor(&self) -> bool {
        self.successor.lock().is_ok_and(|successor| successor.is_some())
    }

    async fn lock_slot(&self) -> OwnedMutexGuard<ResponsesWsSlot> {
        if self.watching.load(Ordering::Acquire) {
            self.wake.notify_one();
        }
        self.slot.clone().lock_owned().await
    }

    pub(crate) async fn close(&self) {
        let mut slot = self.lock_slot().await;
        if let Some(mut session) = slot.session.take() {
            session.close().await;
        }
        slot.forget_session_state();
    }
}

/// WS2h item 6: reads a pinned session between turns so late notifications
/// reach the client attributed to their own response.
async fn watch_idle_session(mut slot: OwnedMutexGuard<ResponsesWsSlot>, lane: Arc<ResponsesWsLane>) {
    loop {
        let Some(session) = slot.session.as_mut() else {
            break;
        };
        tokio::select! {
            biased;
            _ = lane.wake.notified() => break,
            next = session.next_text() => {
                let keep = match next {
                    Ok(Some(text)) => relay_idle_frame(&mut slot, &lane, &text),
                    Ok(None) | Err(_) => false,
                };
                if !keep {
                    if let Some(mut session) = slot.session.take() {
                        session.close().await;
                    }
                    slot.forget_session_state();
                    break;
                }
            }
        }
    }
    lane.watching.store(false, Ordering::Release);
}

/// Returns false when the frame shows the session is no longer idle-consistent.
fn relay_idle_frame(slot: &mut ResponsesWsSlot, lane: &ResponsesWsLane, text: &str) -> bool {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ack = kind.starts_with("response.interrupt.") || kind.starts_with("response.steer.");
    if !ack && (kind.starts_with("response.") || kind == "error") {
        return false;
    }
    capture_turn_state(slot, &value);
    map_known_response_ids(&mut value, &slot.public_ids);
    let _ = lane.relay.send(value);
    true
}

fn capture_turn_state(slot: &mut ResponsesWsSlot, value: &Value) {
    if value.get("type").and_then(Value::as_str) == Some("codex.response.metadata")
        && let Some(turn_state) = value
            .get("headers")
            .and_then(|headers| headers.get(TURN_STATE_HEADER))
            .and_then(Value::as_str)
    {
        slot.turn_state = Some(turn_state.to_string());
    }
}

/// WS19: rewrites `response_id` and `previous_response_id` members naming a
/// known upstream response to that response's public id.
fn map_known_response_ids(value: &mut Value, public_ids: &HashMap<String, String>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if matches!(key.as_str(), "response_id" | "previous_response_id")
                    && let Some(public) = child.as_str().and_then(|id| public_ids.get(id))
                {
                    *child = Value::String(public.clone());
                } else {
                    map_known_response_ids(child, public_ids);
                }
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| map_known_response_ids(item, public_ids)),
        _ => {}
    }
}

/// The first `response_id` member at any depth (e.g. `timing_metrics.response_id`).
fn find_response_id(value: &Value) -> Option<&str> {
    match value {
        Value::Object(object) => object
            .get("response_id")
            .and_then(Value::as_str)
            .or_else(|| object.values().find_map(find_response_id)),
        Value::Array(items) => items.iter().find_map(find_response_id),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct ResponsesWsTurnState {
    upstream_kind: AtomicU8,
    local_warmup: AtomicBool,
    steer_rejected: AtomicBool,
    feeders: std::sync::atomic::AtomicUsize,
    feeder_done: tokio::sync::Notify,
    /// Inputs of accepted steers answered with `response.steer.pending`.
    pending_steers: std::sync::Mutex<Vec<Value>>,
}

impl ResponsesWsTurnState {
    pub(crate) fn take_pending_steers(&self) -> Vec<Value> {
        self.pending_steers
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }
}

/// WS2i: Channels whose upstream WebSocket rejected `response.steer`.
static STEERING_REJECTING_CHANNELS: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

fn channel_rejects_steering(channel_id: &str) -> bool {
    STEERING_REJECTING_CHANNELS
        .lock()
        .is_ok_and(|channels| channels.contains(channel_id))
}

impl ResponsesWsTurnState {
    /// WS2i: waits until every upstream feeder of this turn finished reading,
    /// including steering follow-ups after the terminal.
    pub(crate) async fn wait_feeders(&self) {
        loop {
            let notified = self.feeder_done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.feeders.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    fn feeder_finished(&self) {
        self.feeders.fetch_sub(1, Ordering::AcqRel);
        self.feeder_done.notify_waiters();
    }
    /// WS12b: steering must fail locally for this turn's upstream.
    pub(crate) fn steer_rejected(&self) -> bool {
        self.steer_rejected.load(Ordering::Acquire)
    }

    pub(crate) fn upstream_kind(&self) -> u8 {
        self.upstream_kind.load(Ordering::Acquire)
    }

    pub(crate) fn set_upstream_kind(&self, kind: u8) {
        self.upstream_kind.store(kind, Ordering::Release);
    }

    pub(crate) fn local_warmup(&self) -> bool {
        self.local_warmup.load(Ordering::Acquire)
    }

    pub(crate) fn set_local_warmup(&self) {
        self.local_warmup.store(true, Ordering::Release);
    }
}

/// WS2a: per-turn context the WebSocket handler passes to the Responses pipeline.
#[derive(Clone)]
pub(crate) struct ResponsesWsTurn {
    pub(crate) control: watch::Receiver<ResponsesWsControl>,
    pub(crate) steer: Arc<std::sync::Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
    pub(crate) lane: Arc<ResponsesWsLane>,
    pub(crate) state: Arc<ResponsesWsTurnState>,
    pub(crate) public_response_id: String,
    pub(crate) previous_response_id: Option<String>,
    pub(crate) warmup: bool,
    /// WS18: Channel id and upstream model of the session the successor reads.
    pub(crate) successor: Option<(String, String)>,
    pub(crate) client_metadata: Option<Value>,
    /// WS5a: Monoize appended `reasoning.encrypted_content` to `include`.
    pub(crate) encrypted_include_added: bool,
    /// True once the downstream connection closed; outlives the turn's own
    /// control so post-terminal reading still observes disconnects (WS2g).
    pub(crate) connection_closed: watch::Receiver<bool>,
}

impl ResponsesWsTurn {
    pub(crate) fn is_disconnected(&self) -> bool {
        *self.control.borrow() == ResponsesWsControl::Disconnected
            || *self.connection_closed.borrow()
    }
}

pub(crate) struct UpstreamWsTarget {
    pub(crate) base_url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) proxy: Option<String>,
    pub(crate) timeout_ms: u64,
    pub(crate) channel_id: String,
    pub(crate) upstream_model: String,
}

pub(crate) enum TurnOpenError {
    /// WS2c: no `101`; the untouched encoded body is returned for HTTP fallback.
    Handshake { error: HandshakeError, body: Value },
    Send(String),
}

pub(crate) struct ResponsesWsTurnSource {
    slot: OwnedMutexGuard<ResponsesWsSlot>,
    session: Option<Box<dyn ResponsesWsSession>>,
    target: UpstreamWsTarget,
    /// Full create text kept only while WS2h item 3 may replay it.
    full_text: String,
    full_input: Vec<Value>,
    incremental: bool,
    reused: bool,
}

async fn connect_with_turn_state(
    target: &UpstreamWsTarget,
    turn_state: Option<&str>,
) -> Result<Box<dyn ResponsesWsSession>, HandshakeError> {
    let mut headers = target.headers.clone();
    if let Some(turn_state) = turn_state {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(TURN_STATE_HEADER));
        headers.push((TURN_STATE_HEADER.to_string(), turn_state.to_string()));
    }
    connect_responses_websocket(
        &target.base_url,
        &headers,
        target.proxy.as_deref(),
        target.timeout_ms,
    )
    .await
}

/// WS2h items 1–2 and WS18: obtains the lane session for this attempt and sends
/// the create frame (none for a successor).
pub(crate) async fn open_turn_source(
    turn: &ResponsesWsTurn,
    target: UpstreamWsTarget,
    body: Value,
) -> Result<ResponsesWsTurnSource, TurnOpenError> {
    let mut slot = turn.lane.lock_slot().await;
    if channel_rejects_steering(&target.channel_id) {
        turn.state.steer_rejected.store(true, Ordering::Release);
    }
    let same_route = slot.session.is_some()
        && slot.channel_id == target.channel_id
        && slot.upstream_model == target.upstream_model;
    if turn.successor.is_some() {
        if !same_route || slot.stashed.is_empty() {
            return Err(TurnOpenError::Send(
                "the steering successor session is unavailable".to_string(),
            ));
        }
        let session = slot.session.take();
        let full_input = body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return Ok(ResponsesWsTurnSource {
            slot,
            session,
            target,
            full_text: String::new(),
            full_input,
            incremental: false,
            reused: false,
        });
    }
    if slot.turn_state_channel != target.channel_id {
        slot.turn_state = None;
        slot.turn_state_channel = target.channel_id.clone();
    }
    // A stashed successor belongs to its WS18 turn only; an ordinary turn must
    // never read its frames, so that session is replaced.
    let same_route = same_route && slot.stashed.is_empty();
    let (mut session, reused) = match slot.session.take() {
        Some(session) if same_route => (session, true),
        held => {
            if let Some(mut held) = held {
                held.close().await;
            }
            slot.forget_session_state();
            match connect_with_turn_state(&target, slot.turn_state.as_deref()).await {
                Ok(session) => (session, false),
                Err(error) => return Err(TurnOpenError::Handshake { error, body }),
            }
        }
    };
    if !reused && let Some(turn_state) = session.handshake_turn_state() {
        slot.turn_state = Some(turn_state);
    }

    let mut payload = responses_ws_create_payload(body);
    if let Some(object) = payload.as_object_mut() {
        if turn.warmup {
            object.insert("generate".to_string(), Value::Bool(false));
        }
        if !object.contains_key("client_metadata")
            && let Some(metadata) = turn.client_metadata.clone()
        {
            object.insert("client_metadata".to_string(), metadata);
        }
        if let Some(turn_state) = slot.turn_state.clone() {
            let metadata = object
                .entry("client_metadata")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(metadata) = metadata.as_object_mut()
                && !metadata.contains_key(TURN_STATE_HEADER)
            {
                metadata.insert(TURN_STATE_HEADER.to_string(), Value::String(turn_state));
            }
        }
    }
    let full_input = payload
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let full_text = payload.to_string();
    let incremental_text = if reused {
        incremental_create(&slot, turn, &payload, &full_input)
    } else {
        None
    };
    drop(payload);
    let incremental = incremental_text.is_some();
    // WS2h item 2: the session still queues pending steering, which a full create
    // already contains, so a full replay must not run on it.
    let sent = if reused && !incremental && slot.pending_steer_parts > 0 {
        Err("pending steering needs a fresh session".to_string())
    } else {
        session
            .send_text(incremental_text.unwrap_or_else(|| full_text.clone()))
            .await
    };
    match sent {
        Ok(()) => Ok(ResponsesWsTurnSource {
            slot,
            session: Some(session),
            target,
            full_text: if reused { full_text } else { String::new() },
            full_input,
            incremental,
            reused,
        }),
        Err(_) if reused => {
            // WS2h item 3: a stale pinned session is replaced transparently.
            session.close().await;
            slot.last_upstream_id = None;
            let mut session = connect_with_turn_state(&target, slot.turn_state.as_deref())
                .await
                .map_err(|error| TurnOpenError::Send(error.message))?;
            session
                .send_text(full_text)
                .await
                .map_err(TurnOpenError::Send)?;
            Ok(ResponsesWsTurnSource {
                slot,
                session: Some(session),
                target,
                full_text: String::new(),
                full_input,
                incremental: false,
                reused: false,
            })
        }
        Err(error) => Err(TurnOpenError::Send(error)),
    }
}

fn content_parts(content: &Value) -> usize {
    content.as_array().map_or(1, Vec::len)
}

/// WS2h item 2: content parts a steer input contributes to the encoded history.
fn steer_content_parts(input: &Value) -> usize {
    match input {
        Value::Array(items) => items
            .iter()
            .map(|item| item.get("content").map_or(1, content_parts))
            .sum(),
        _ => 1,
    }
}

/// WS2h item 2: the incremental create text, when the encoded input extends the
/// session basis and the turn continues the session's last public response.
fn incremental_create(
    slot: &ResponsesWsSlot,
    turn: &ResponsesWsTurn,
    payload: &Value,
    full_input: &[Value],
) -> Option<String> {
    let upstream_id = slot.last_upstream_id.as_ref()?;
    let previous = turn.previous_response_id.as_ref()?;
    if slot.last_public_id.as_ref() != Some(previous) {
        return None;
    }
    let basis = slot.basis.len();
    let replayed = basis.saturating_add(slot.last_output.len());
    if full_input.len() < replayed || full_input[..basis] != slot.basis[..] {
        return None;
    }
    // The replayed previous output must be exactly the upstream's own output
    // items, proven by item id; anything else needs the full create.
    for ((kind, id), item) in slot.last_output.iter().zip(&full_input[basis..replayed]) {
        let id = id.as_deref()?;
        let item_kind = item.get("type").and_then(Value::as_str).unwrap_or("message");
        if item_kind != kind || item.get("id").and_then(Value::as_str) != Some(id) {
            return None;
        }
    }
    // The encoder merges adjacent user messages, so pending steering must span
    // whole items by content part count; a merge with new input needs the full create.
    let mut prefix = replayed;
    let mut parts = 0;
    while parts < slot.pending_steer_parts {
        let item = full_input.get(prefix)?;
        if item.get("role").and_then(Value::as_str) != Some("user") {
            return None;
        }
        parts += item.get("content").map_or(1, content_parts);
        prefix += 1;
    }
    if parts != slot.pending_steer_parts {
        return None;
    }
    let mut object = serde_json::Map::new();
    for (key, value) in payload.as_object()? {
        if key != "input" {
            object.insert(key.clone(), value.clone());
        }
    }
    object.insert(
        "input".to_string(),
        Value::Array(full_input[prefix..].to_vec()),
    );
    object.insert(
        "previous_response_id".to_string(),
        Value::String(upstream_id.clone()),
    );
    Some(Value::Object(object).to_string())
}

/// WS2: synthetic warmup frames fed through the stream decoder when a warmup
/// turn cannot reach an upstream WebSocket (WS2c, WS6).
pub(crate) fn local_warmup_frames(model: &str) -> Vec<String> {
    let id = format!("resp_monoize_warmup_{}", uuid::Uuid::new_v4().simple());
    let created_at = chrono::Utc::now().timestamp();
    let response = |status: &str, usage: Value| {
        json!({
            "id": id,
            "object": "response",
            "created_at": created_at,
            "status": status,
            "model": model,
            "output": [],
            "usage": usage,
        })
    };
    vec![
        json!({
            "type": "response.created",
            "sequence_number": 0,
            "response": response("in_progress", Value::Null),
        })
        .to_string(),
        json!({
            "type": "response.completed",
            "sequence_number": 1,
            "response": response(
                "completed",
                json!({"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}),
            ),
        })
        .to_string(),
    ]
}

type FrameSender = mpsc::Sender<Result<(String, String), AppError>>;

/// WS12b shape of a steer that the routed upstream cannot apply.
fn steering_not_supported(public_response_id: &str, input: Value) -> Value {
    json!({
        "type": "response.steer.failed",
        "steer": {
            "id": format!("steer_{}", uuid::Uuid::new_v4().simple()),
            "previous_response_id": public_response_id,
            "input": input,
        },
        "error": {
            "type": "invalid_request_error",
            "code": "steering_not_supported",
            "message": "The routed upstream does not support steering; resend the input with response.create.",
        },
    })
}

enum FeedFlow {
    Continue,
    Finish,
    Replay,
}

/// Reads one turn's upstream frames into the stream decoder (WS2d) and handles
/// interruption (WS2g), session reuse fallback (WS2h), and steering (WS2i).
struct TurnFeeder {
    source: ResponsesWsTurnSource,
    turn: ResponsesWsTurn,
    frames: FrameSender,
    idle: Duration,
    upstream_id: Option<String>,
    terminal_id: Option<String>,
    terminal_output: Vec<(String, Option<String>)>,
    pending_steer_parts: usize,
    terminal: bool,
    got_frame: bool,
    decided: bool,
    replayed: bool,
    buffered: Vec<(String, String)>,
    interrupt_requested: bool,
    interrupt_sent: bool,
    control_closed: bool,
    connection_seen: bool,
    decoder_gone: bool,
    pending_steers: VecDeque<Value>,
    sent_steers: VecDeque<Value>,
    accepted_steers: Vec<(String, Value)>,
    successor: bool,
}

pub(crate) fn spawn_turn_feeder(
    source: ResponsesWsTurnSource,
    turn: ResponsesWsTurn,
    frames: FrameSender,
    idle_timeout_ms: u64,
) {
    let feeder = TurnFeeder {
        source,
        turn,
        frames,
        idle: Duration::from_millis(idle_timeout_ms.max(1)),
        upstream_id: None,
        terminal_id: None,
        terminal_output: Vec::new(),
        pending_steer_parts: 0,
        terminal: false,
        got_frame: false,
        decided: false,
        replayed: false,
        buffered: Vec::new(),
        interrupt_requested: false,
        interrupt_sent: false,
        control_closed: false,
        connection_seen: false,
        decoder_gone: false,
        pending_steers: VecDeque::new(),
        sent_steers: VecDeque::new(),
        accepted_steers: Vec::new(),
        successor: false,
    };
    feeder.turn.state.feeders.fetch_add(1, Ordering::AcqRel);
    tokio::spawn(feeder.run());
}

async fn recv_steer(rx: &mut Option<mpsc::UnboundedReceiver<Value>>) -> Option<Value> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

impl TurnFeeder {
    async fn run(mut self) {
        let mut steer_rx = self.turn.steer.lock().ok().and_then(|mut rx| rx.take());
        if self.turn.is_disconnected() || *self.turn.control.borrow() != ResponsesWsControl::Running {
            self.interrupt_requested = true;
        }
        let keep = 'feed: {
            for text in std::mem::take(&mut self.source.slot.stashed) {
                match self.feed_frame(text).await {
                    FeedFlow::Finish => break 'feed true,
                    FeedFlow::Continue | FeedFlow::Replay => {}
                }
            }
            loop {
                let Some(session) = self.source.session.as_mut() else {
                    break 'feed false;
                };
                let next = tokio::select! {
                    biased;
                    changed = self.turn.control.changed(), if !self.interrupt_requested && !self.control_closed => {
                        match changed {
                            // The handler drops the sender when the downstream turn
                            // ends; that is not an interruption.
                            Err(_) => self.control_closed = true,
                            Ok(()) => {
                                if *self.turn.control.borrow() != ResponsesWsControl::Running {
                                    self.interrupt_requested = true;
                                    self.send_interrupt_if_ready().await;
                                }
                            }
                        }
                        continue;
                    }
                    changed = self.turn.connection_closed.changed(), if !self.connection_seen => {
                        if changed.is_err() || *self.turn.connection_closed.borrow() {
                            self.connection_seen = true;
                            if self.terminal {
                                // Only steering follow-ups remain; nobody can receive them.
                                break 'feed false;
                            }
                            self.interrupt_requested = true;
                            self.send_interrupt_if_ready().await;
                        }
                        continue;
                    }
                    steer = recv_steer(&mut steer_rx), if steer_rx.is_some() => {
                        match steer {
                            Some(steer) => {
                                self.pending_steers.push_back(steer);
                                self.send_steers_if_ready().await;
                            }
                            None => steer_rx = None,
                        }
                        continue;
                    }
                    next = timeout(self.idle, session.next_text()) => next,
                };
                match next {
                    Err(_) => {
                        if !self.terminal {
                            self.fail(
                                StatusCode::GATEWAY_TIMEOUT,
                                "upstream_idle_timeout",
                                format!(
                                    "upstream stream idle for {}ms without data",
                                    self.idle.as_millis()
                                ),
                            )
                            .await;
                        }
                        break 'feed false;
                    }
                    Ok(Ok(Some(text))) => match self.feed_frame(text).await {
                        FeedFlow::Continue => {}
                        FeedFlow::Finish => break 'feed true,
                        FeedFlow::Replay => {
                            if !self.replay().await {
                                break 'feed false;
                            }
                        }
                    },
                    Ok(Ok(None)) | Ok(Err(_)) => {
                        if !self.got_frame && self.source.reused && !self.replayed {
                            if self.replay().await {
                                continue;
                            }
                            break 'feed false;
                        }
                        if !self.terminal {
                            self.fail(
                                StatusCode::BAD_GATEWAY,
                                "upstream_stream_decode_failed",
                                "upstream WebSocket closed before a terminal event".to_string(),
                            )
                            .await;
                        }
                        break 'feed false;
                    }
                }
            }
        };
        self.finish(keep && self.terminal).await;
        if let Ok(mut slot) = self.turn.steer.lock()
            && let Some(rx) = steer_rx
        {
            *slot = Some(rx);
        }
        let state = self.turn.state.clone();
        let lane = self.turn.lane.clone();
        // A stashed successor is read by its own turn, not by the idle watcher.
        if self.source.slot.session.is_some() && self.source.slot.stashed.is_empty() {
            lane.watching.store(true, Ordering::Release);
            tokio::spawn(watch_idle_session(self.source.slot, lane));
        }
        state.feeder_finished();
    }

    async fn fail(&mut self, status: StatusCode, code: &str, message: String) {
        let _ = self
            .frames
            .send(Err(AppError::new(status, code.to_string(), message)))
            .await;
    }

    fn public_id_for(&self, upstream_id: &str) -> Option<String> {
        self.source.slot.public_ids.get(upstream_id).cloned()
    }

    fn relay(&self, value: Value) {
        let _ = self.turn.lane.relay.send(value);
    }

    async fn feed_frame(&mut self, text: String) -> FeedFlow {
        self.got_frame = true;
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
            if !self.terminal {
                self.forward(String::new(), text).await;
            }
            return FeedFlow::Continue;
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        capture_turn_state(&mut self.source.slot, &value);
        if !kind.starts_with("response.")
            && kind != "error"
            && let Some(id) = find_response_id(&value)
            && self.source.slot.public_ids.contains_key(id)
            && self.upstream_id.as_deref() != Some(id)
        {
            // A late notification of an earlier response on this session keeps
            // its own public id and never enters this turn's decoder.
            map_known_response_ids(&mut value, &self.source.slot.public_ids);
            self.relay(value);
            return FeedFlow::Continue;
        }
        match kind.as_str() {
            "response.interrupt.accepted" | "response.interrupt.failed" => {
                if kind == "response.interrupt.failed"
                    && value.get("response_id").and_then(Value::as_str) == self.upstream_id.as_deref()
                {
                    self.signal_discard_requested(false).await;
                }
                if !self.turn.is_disconnected() {
                    if let Some(public) = value
                        .get("response_id")
                        .and_then(Value::as_str)
                        .and_then(|id| self.public_id_for(id))
                    {
                        value["response_id"] = Value::String(public);
                    }
                    self.relay(value);
                }
                return FeedFlow::Continue;
            }
            "response.steer.accepted" | "response.steer.pending" | "response.steer.failed" => {
                let steer_id = value
                    .pointer("/steer/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if kind == "response.steer.accepted" {
                    let input = self.sent_steers.pop_front().unwrap_or(Value::Null);
                    if let Some(steer_id) = steer_id {
                        self.accepted_steers.push((steer_id, input));
                    }
                } else {
                    let was_accepted = steer_id.as_ref().is_some_and(|steer_id| {
                        self.accepted_steers.iter().any(|(id, _)| id == steer_id)
                    });
                    if was_accepted {
                        if kind == "response.steer.pending"
                            && let Some(position) = self
                                .accepted_steers
                                .iter()
                                .position(|(id, _)| Some(id) == steer_id.as_ref())
                        {
                            // The upstream prepends this input to the next create;
                            // full replays must contain it too (WS7, WS2h item 2).
                            let (_, input) = self.accepted_steers.remove(position);
                            self.pending_steer_parts += steer_content_parts(&input);
                            if let Ok(mut pending) = self.turn.state.pending_steers.lock() {
                                pending.push(input);
                            }
                        }
                        self.accepted_steers
                            .retain(|(id, _)| Some(id) != steer_id.as_ref());
                    } else {
                        // A rejection without prior acceptance answers the oldest
                        // unacknowledged sent steer.
                        self.sent_steers.pop_front();
                    }
                }
                if let Some(public) = value
                    .pointer("/steer/previous_response_id")
                    .and_then(Value::as_str)
                    .and_then(|id| self.public_id_for(id))
                {
                    value["steer"]["previous_response_id"] = Value::String(public);
                }
                self.relay(value);
                return if self.terminal && self.steers_resolved() {
                    FeedFlow::Finish
                } else {
                    FeedFlow::Continue
                };
            }
            _ => {}
        }
        if kind == "error"
            && value.get("response").is_none()
            && !self.sent_steers.is_empty()
            && matches!(
                value.pointer("/error/code").and_then(Value::as_str),
                Some("invalid_request_type" | "unsupported_request_type" | "unsupported_event_type")
            )
        {
            // WS2i: an upstream without steering rejects the event type while its
            // response keeps streaming; this error belongs to the steer.
            let input = self.sent_steers.pop_front().unwrap_or(Value::Null);
            self.turn.state.steer_rejected.store(true, Ordering::Release);
            if let Ok(mut channels) = STEERING_REJECTING_CHANNELS.lock() {
                channels.insert(self.source.target.channel_id.clone());
            }
            self.relay(steering_not_supported(&self.turn.public_response_id, input));
            return if self.terminal && self.steers_resolved() {
                FeedFlow::Finish
            } else {
                FeedFlow::Continue
            };
        }
        if self.terminal {
            if kind == "response.created" && !self.accepted_steers.is_empty() {
                self.source.slot.stashed.push(text);
                self.successor = true;
                return FeedFlow::Finish;
            }
            if !kind.starts_with("response.") && kind != "error" {
                map_known_response_ids(&mut value, &self.source.slot.public_ids);
                self.relay(value);
            }
            return FeedFlow::Continue;
        }
        if !self.decided && self.source.reused && !self.replayed {
            let incremental_miss = kind == "error"
                && self.source.incremental
                && value.pointer("/error/code").and_then(Value::as_str)
                    == Some("previous_response_not_found");
            if incremental_miss {
                return FeedFlow::Replay;
            }
            if !kind.starts_with("response.") && kind != "error" {
                self.buffered.push((kind, text));
                return FeedFlow::Continue;
            }
            self.decided = true;
            for (kind, text) in std::mem::take(&mut self.buffered) {
                self.forward(kind, text).await;
            }
        }
        if kind == "response.created"
            && let Some(id) = value.pointer("/response/id").and_then(Value::as_str)
        {
            self.upstream_id = Some(id.to_string());
            self.source
                .slot
                .public_ids
                .insert(id.to_string(), self.turn.public_response_id.clone());
            self.send_interrupt_if_ready().await;
            self.send_steers_if_ready().await;
        }
        let terminal = matches!(
            kind.as_str(),
            "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "error"
        );
        if terminal {
            self.terminal = true;
            self.terminal_id = value
                .pointer("/response/id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| self.upstream_id.clone());
            self.terminal_output = value
                .pointer("/response/output")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            (
                                item.get("type")
                                    .and_then(Value::as_str)
                                    .unwrap_or("message")
                                    .to_string(),
                                item.get("id").and_then(Value::as_str).map(str::to_string),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        drop(value);
        self.forward(kind, text).await;
        if self.terminal && self.steers_resolved() {
            FeedFlow::Finish
        } else {
            FeedFlow::Continue
        }
    }

    fn steers_resolved(&self) -> bool {
        self.accepted_steers.is_empty() && self.sent_steers.is_empty()
    }

    async fn forward(&mut self, kind: String, text: String) {
        if self.decoder_gone {
            return;
        }
        if self.frames.send(Ok((kind, text))).await.is_err() {
            // The pipeline stopped reading; stop upstream work but keep reading
            // to the terminal so the session stays reusable.
            self.decoder_gone = true;
            self.interrupt_requested = true;
            self.send_interrupt_if_ready().await;
        }
    }

    async fn send_interrupt_if_ready(&mut self) {
        if !self.interrupt_requested || self.interrupt_sent || self.terminal {
            return;
        }
        let (Some(upstream_id), Some(session)) =
            (self.upstream_id.clone(), self.source.session.as_mut())
        else {
            return;
        };
        self.interrupt_sent = true;
        let frame = json!({
            "type": "response.interrupt",
            "response_id": upstream_id,
            "mode": "discard_partial_items",
        });
        match timeout(self.idle, session.send_text(frame.to_string())).await {
            Ok(Ok(())) => self.signal_discard_requested(true).await,
            Ok(Err(error)) => {
                tracing::warn!(%error, "upstream WebSocket interrupt send failed; draining usage")
            }
            Err(_) => tracing::warn!("upstream WebSocket interrupt send timed out; draining usage"),
        }
    }

    /// WS2g: while true, the decoder treats the terminal `output` as authoritative
    /// (partial items discarded upstream). Sent on interrupt send, not on
    /// acknowledgement, because some upstreams never acknowledge.
    async fn signal_discard_requested(&mut self, requested: bool) {
        if self.decoder_gone {
            return;
        }
        let _ = self
            .frames
            .send(Ok((
                crate::urp::RESPONSES_DISCARD_REQUESTED_FRAME.to_string(),
                requested.to_string(),
            )))
            .await;
    }

    async fn send_steers_if_ready(&mut self) {
        let (Some(upstream_id), Some(session)) =
            (self.upstream_id.clone(), self.source.session.as_mut())
        else {
            return;
        };
        while let Some(input) = self.pending_steers.pop_front() {
            let frame = json!({
                "type": "response.steer",
                "previous_response_id": upstream_id,
                "input": input,
            });
            match timeout(self.idle, session.send_text(frame.to_string())).await {
                Ok(Ok(())) => self.sent_steers.push_back(input),
                _ => {
                    let _ = self.turn.lane.relay.send(json!({
                        "type": "response.steer.failed",
                        "steer": {
                            "id": format!("steer_{}", uuid::Uuid::new_v4().simple()),
                            "previous_response_id": self.turn.public_response_id,
                            "input": input,
                        },
                        "error": {
                            "type": "invalid_request_error",
                            "code": "response_not_active",
                            "message": "This response no longer accepts steering input.",
                        },
                    }));
                }
            }
        }
    }

    /// WS2h item 3: replace the reused session and resend the full create once.
    async fn replay(&mut self) -> bool {
        self.replayed = true;
        if let Some(mut session) = self.source.session.take() {
            session.close().await;
        }
        self.source.slot.last_upstream_id = None;
        let turn_state = self.source.slot.turn_state.clone();
        let mut session = match connect_with_turn_state(&self.source.target, turn_state.as_deref())
            .await
        {
            Ok(session) => session,
            Err(error) => {
                self.fail(
                    StatusCode::BAD_GATEWAY,
                    "upstream_websocket_handshake_failed",
                    error.message,
                )
                .await;
                return false;
            }
        };
        if let Some(turn_state) = session.handshake_turn_state() {
            self.source.slot.turn_state = Some(turn_state);
        }
        let text = std::mem::take(&mut self.source.full_text);
        if let Err(error) = session.send_text(text).await {
            self.fail(
                StatusCode::BAD_GATEWAY,
                "upstream_websocket_send_failed",
                error,
            )
            .await;
            return false;
        }
        self.source.session = Some(session);
        self.source.incremental = false;
        self.buffered.clear();
        self.decided = false;
        self.got_frame = false;
        self.upstream_id = None;
        true
    }

    async fn finish(&mut self, keep: bool) {
        let slot = &mut self.source.slot;
        if keep && !self.turn.is_disconnected() {
            slot.session = self.source.session.take();
            slot.channel_id = self.source.target.channel_id.clone();
            slot.upstream_model = self.source.target.upstream_model.clone();
            slot.last_upstream_id = self.terminal_id.clone();
            slot.last_public_id = Some(self.turn.public_response_id.clone());
            slot.basis = std::mem::take(&mut self.source.full_input);
            slot.last_output = std::mem::take(&mut self.terminal_output);
            slot.pending_steer_parts = self.pending_steer_parts;
        } else {
            if let Some(mut session) = self.source.session.take() {
                session.close().await;
            }
            slot.forget_session_state();
            self.successor = false;
        }
        if self.successor
            && let Ok(mut successor) = self.turn.lane.successor.lock()
        {
            *successor = Some(ResponsesWsSuccessor {
                steer_inputs: std::mem::take(&mut self.accepted_steers)
                    .into_iter()
                    .map(|(_, input)| input)
                    .collect(),
                channel_id: self.source.target.channel_id.clone(),
                upstream_model: self.source.target.upstream_model.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_authority_brackets_ipv6() {
        assert_eq!(connect_authority("::1", 443), "[::1]:443");
        assert_eq!(connect_authority("[::1]", 8080), "[::1]:8080");
        assert_eq!(
            connect_authority("proxy.example", 3128),
            "proxy.example:3128"
        );
    }

    #[test]
    fn connect_header_end_keeps_leftover_offset() {
        let mut buf = b"HTTP/1.1 200 Connection Established\r\n\r\nABC".to_vec();
        let end = connect_header_end(&buf).expect("200");
        assert_eq!(&buf[end..], b"ABC");
        buf = b"HTTP/1.1 502 Bad Gateway\r\n\r\n".to_vec();
        let err = connect_header_end(&buf).expect_err("502");
        assert_eq!(err.http_status, Some(502));
    }

    #[test]
    fn responses_ws_create_payload_strips_transport_fields() {
        let body = serde_json::json!({
            "model": "gpt-5",
            "stream": true,
            "background": false,
            "generate": true,
            "previous_response_id": "resp_old",
            "type": "response.create",
            "input": []
        });
        let payload = responses_ws_create_payload(body);
        let object = payload.as_object().unwrap();
        assert_eq!(
            object.get("type").and_then(serde_json::Value::as_str),
            Some("response.create")
        );
        assert!(!object.contains_key("stream"));
        assert!(!object.contains_key("background"));
        assert!(!object.contains_key("generate"));
        assert!(!object.contains_key("previous_response_id"));
        assert_eq!(
            object.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5")
        );
    }

    #[test]
    fn rustls_client_config_installs_crypto_provider() {
        ensure_rustls_crypto_provider();
        let _ = rustls_client_config();
    }
}

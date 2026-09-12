use crate::error::AppError;
use crate::upstream::join_url;
use async_trait::async_trait;
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
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
}

#[async_trait]
impl ResponsesWsSession for TungsteniteResponsesWs {
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
        })
    } else {
        let (stream, response) =
            client_async_with_config(request, tunneled, Some(upstream_ws_config()))
                .await
                .map_err(classify_connect_error)?;
        ensure_switching_protocols(response.status().as_u16())?;
        Ok(TungsteniteResponsesWs {
            inner: InnerWs::Plain(stream),
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

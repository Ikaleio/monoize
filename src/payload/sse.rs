use super::{CHUNK, JsonInput, Payload, ReadError, SpoolWriter, WIRE_LIMIT, write_json};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct Frame {
    pub(crate) event: String,
    pub(crate) data: FrameData,
}

pub(crate) enum FrameData {
    Text(String),
    Json(Result<Value, ReadError>),
}

impl FrameData {
    pub(crate) fn is_done(&self) -> bool {
        matches!(self, Self::Text(text) if text.trim() == "[DONE]")
    }

    pub(crate) fn into_json(self) -> Result<Value, (String, String)> {
        match self {
            Self::Text(text) => {
                serde_json::from_str(&text).map_err(|error| (error.to_string(), text))
            }
            Self::Json(result) => result.map_err(|error| (error.to_string(), String::new())),
        }
    }
}

impl From<String> for FrameData {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SseError {
    #[error("upstream stream idle for {0}ms without data")]
    Idle(u64),
    #[error("{0}")]
    Decode(String),
}

impl SseError {
    pub(crate) fn into_app(self) -> crate::error::AppError {
        let (status, code) = match self {
            Self::Idle(_) => (
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "upstream_idle_timeout",
            ),
            Self::Decode(_) => (
                axum::http::StatusCode::BAD_GATEWAY,
                "upstream_stream_decode_failed",
            ),
        };
        crate::error::AppError::new(status, code, self.to_string())
    }
}

pub(crate) struct Frames {
    chunks:
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    payload: Arc<Payload>,
    chunk: Bytes,
    idle_ms: u64,
    bytes: usize,
    skip_lf: bool,
    utf8_tail: Vec<u8>,
    line: Line,
    name: String,
    data: Data,
    has_data: bool,
    text_only: bool,
    first: bool,
}

enum Line {
    Start(Vec<u8>),
    Data { skip_space: bool },
    Event(Vec<u8>),
    Ignore,
}

#[derive(Default)]
enum Data {
    #[default]
    Empty,
    Text(Vec<u8>),
    Json(Result<Box<JsonInput>, ReadError>),
    Spool(Box<SpoolWriter>),
}

impl Frames {
    pub(crate) fn new(response: reqwest::Response, idle_ms: u64, text_only: bool) -> Self {
        Self {
            chunks: Box::pin(response.bytes_stream()),
            payload: super::current().unwrap_or_default(),
            chunk: Bytes::new(),
            idle_ms,
            bytes: 0,
            skip_lf: false,
            utf8_tail: Vec::new(),
            line: Line::Start(Vec::new()),
            name: String::new(),
            data: Data::Empty,
            has_data: false,
            text_only,
            first: true,
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Frame>, SseError> {
        loop {
            if self.chunk.is_empty() {
                let timeout = Duration::from_millis(self.idle_ms.max(1));
                self.chunk = match tokio::time::timeout(timeout, self.chunks.next()).await {
                    Err(_) => return Err(SseError::Idle(self.idle_ms)),
                    Ok(None) if !self.utf8_tail.is_empty() => {
                        return Err(SseError::Decode("incomplete UTF-8 in upstream SSE".into()));
                    }
                    Ok(None) => return Ok(None),
                    Ok(Some(Err(error))) => {
                        return Err(SseError::Decode(crate::error::error_chain(&error)));
                    }
                    Ok(Some(Ok(chunk))) => chunk,
                };
                check_utf8(&mut self.utf8_tail, &self.chunk)?;
                if self.chunk.is_empty() {
                    continue;
                }
            }
            let mut chunk = std::mem::take(&mut self.chunk);
            if std::mem::take(&mut self.skip_lf) && chunk.first() == Some(&b'\n') {
                let _ = chunk.split_to(1);
                if self.bytes != 0 {
                    self.bytes = self.bytes.saturating_add(1);
                }
                if chunk.is_empty() {
                    continue;
                }
            }
            let end = memchr::memchr2(b'\r', b'\n', &chunk).unwrap_or(chunk.len());
            let finished = end < chunk.len();
            let line = chunk.split_to(end);
            self.bytes = self.bytes.saturating_add(end + usize::from(finished));
            if self.bytes > WIRE_LIMIT {
                return Err(SseError::Decode(format!(
                    "upstream SSE event exceeds {WIRE_LIMIT} bytes"
                )));
            }
            self.feed_line(&line)?;
            if finished {
                self.skip_lf = chunk.split_to(1)[0] == b'\r';
            }
            self.chunk = chunk;
            if finished && let Some(frame) = self.end_line()? {
                return Ok(Some(frame));
            }
        }
    }

    fn feed_line(&mut self, bytes: &[u8]) -> Result<(), SseError> {
        match &mut self.line {
            Line::Start(prefix) => {
                if bytes.is_empty() {
                    return Ok(());
                }
                let end = memchr::memchr(b':', bytes).unwrap_or(bytes.len());
                let max_prefix = if self.first { 8 } else { 5 };
                if prefix.len() + end > max_prefix {
                    self.line = Line::Ignore;
                    return Ok(());
                }
                prefix.extend_from_slice(&bytes[..end]);
                if end < bytes.len() {
                    let prefix = prefix
                        .strip_prefix(b"\xef\xbb\xbf")
                        .filter(|_| self.first)
                        .unwrap_or(prefix);
                    self.line = match prefix {
                        b"data" => {
                            self.start_data()?;
                            Line::Data { skip_space: true }
                        }
                        b"event" => Line::Event(Vec::new()),
                        _ => Line::Ignore,
                    };
                    self.feed_line(&bytes[end + 1..])?;
                }
            }
            Line::Data { skip_space } => {
                if !bytes.is_empty() {
                    let skip = std::mem::take(skip_space) && bytes[0] == b' ';
                    self.push_data(&bytes[usize::from(skip)..])?;
                }
            }
            Line::Event(name) => {
                if name.len() + bytes.len() > CHUNK {
                    return Err(SseError::Decode(
                        "upstream SSE event name is too long".into(),
                    ));
                }
                name.extend_from_slice(bytes);
            }
            Line::Ignore => {}
        }
        Ok(())
    }

    fn start_data(&mut self) -> Result<(), SseError> {
        if std::mem::replace(&mut self.has_data, true) {
            self.push_data(b"\n")?;
        } else {
            self.data = Data::Text(Vec::new());
        }
        Ok(())
    }

    fn push_data(&mut self, bytes: &[u8]) -> Result<(), SseError> {
        let spool_error = |error: io::Error| SseError::Decode(format!("payload spool: {error}"));
        match &mut self.data {
            Data::Text(text) if text.len() + bytes.len() <= CHUNK => text.extend_from_slice(bytes),
            Data::Text(text) if self.text_only => {
                let mut spool = SpoolWriter::new().map_err(spool_error)?;
                spool
                    .write_all(text)
                    .and_then(|()| spool.write_all(bytes))
                    .map_err(spool_error)?;
                self.data = Data::Spool(Box::new(spool));
            }
            Data::Text(text) => {
                let mut input = JsonInput::new(self.payload.clone());
                let result = input.push(text).and_then(|()| input.push(bytes));
                if let Err(ReadError::Spool(error)) = result {
                    return Err(spool_error(error));
                }
                self.data = Data::Json(result.map(|()| Box::new(input)));
            }
            Data::Json(result) => {
                if let Ok(input) = result
                    && let Err(error) = input.push(bytes)
                {
                    if let ReadError::Spool(error) = error {
                        return Err(spool_error(error));
                    }
                    *result = Err(error);
                }
            }
            Data::Spool(spool) => spool.write_all(bytes).map_err(spool_error)?,
            Data::Empty => unreachable!("an SSE data line initializes the accumulator"),
        }
        Ok(())
    }

    fn end_line(&mut self) -> Result<Option<Frame>, SseError> {
        let first = std::mem::take(&mut self.first);
        match std::mem::replace(&mut self.line, Line::Start(Vec::new())) {
            Line::Start(prefix) if prefix.is_empty() || (first && prefix == b"\xef\xbb\xbf") => {
                self.bytes = 0;
                let mut event = std::mem::take(&mut self.name);
                if event.is_empty() {
                    event = "message".to_owned();
                }
                if !std::mem::take(&mut self.has_data) {
                    return Ok(None);
                }
                let data = match std::mem::take(&mut self.data) {
                    Data::Text(bytes) => FrameData::Text(
                        String::from_utf8(bytes)
                            .map_err(|error| SseError::Decode(error.to_string()))?,
                    ),
                    Data::Json(input) => FrameData::Json(input.and_then(|input| input.finish())),
                    Data::Spool(spool) => FrameData::Text(
                        self.payload.insert(
                            spool
                                .finish()
                                .map_err(|error| SseError::Decode(error.to_string()))?,
                        ),
                    ),
                    Data::Empty => unreachable!("an SSE event with data has an accumulator"),
                };
                return Ok(Some(Frame { event, data }));
            }
            Line::Start(prefix) if prefix == b"data" => self.start_data()?,
            Line::Start(prefix) if prefix == b"event" => self.name.clear(),
            Line::Event(name) => {
                let name = name.strip_prefix(b" ").unwrap_or(&name);
                self.name = std::str::from_utf8(name)
                    .map_err(|error| SseError::Decode(error.to_string()))?
                    .to_owned();
            }
            _ => {}
        }
        Ok(None)
    }
}

fn check_utf8(tail: &mut Vec<u8>, mut bytes: &[u8]) -> Result<(), SseError> {
    let invalid = |error: std::str::Utf8Error| SseError::Decode(error.to_string());
    while !tail.is_empty() {
        let Some((&byte, rest)) = bytes.split_first() else {
            return Ok(());
        };
        bytes = rest;
        tail.push(byte);
        match std::str::from_utf8(tail) {
            Ok(_) => tail.clear(),
            Err(error) if error.error_len().is_none() => {}
            Err(error) => return Err(invalid(error)),
        }
    }
    if let Err(error) = std::str::from_utf8(bytes) {
        if error.error_len().is_some() {
            return Err(invalid(error));
        }
        tail.extend_from_slice(&bytes[error.valid_up_to()..]);
    }
    Ok(())
}

pub(crate) const HEARTBEAT: &[u8] = b": heartbeat\n\n";
pub(crate) const PING: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";

pub(crate) enum WireEvent {
    Bytes(Bytes),
    File(Arc<super::Spool>),
}

impl WireEvent {
    pub(crate) fn plain(data: &str) -> Self {
        let mut frame = Vec::new();
        for line in data.split('\n') {
            frame.extend_from_slice(b"data: ");
            frame.extend_from_slice(line.as_bytes());
            frame.push(b'\n');
        }
        frame.push(b'\n');
        Self::Bytes(frame.into())
    }

    pub(crate) fn json_with_payload(
        name: Option<&str>,
        data: &Value,
        payload: Option<&Payload>,
    ) -> io::Result<Self> {
        if payload.is_some() {
            let mut spool = SpoolWriter::output()?;
            write_frame(&mut spool, name, data, payload)?;
            Ok(Self::File(Arc::new(spool.finish()?)))
        } else {
            let mut frame = Vec::new();
            write_frame(&mut frame, name, data, None)?;
            Ok(Self::Bytes(frame.into()))
        }
    }
}

fn write_frame(
    writer: &mut impl Write,
    name: Option<&str>,
    data: &Value,
    payload: Option<&Payload>,
) -> io::Result<()> {
    if let Some(name) = name {
        writeln!(writer, "event: {name}")?;
    }
    writer.write_all(b"data: ")?;
    write_json(writer, data, payload)?;
    writer.write_all(b"\n\n")
}

pub(crate) fn response<S>(events: S, heartbeat: &'static [u8]) -> axum::response::Response
where
    S: futures_util::Stream<Item = Result<WireEvent, std::convert::Infallible>> + Send + 'static,
{
    use axum::response::IntoResponse;
    let events = events.boxed();
    let chunks = futures_util::stream::try_unfold(
        (events, None),
        move |(mut events, mut file): (
            _,
            Option<futures_util::stream::BoxStream<'static, io::Result<Bytes>>>,
        )| async move {
            loop {
                if let Some(stream) = &mut file {
                    if let Some(chunk) = stream.next().await {
                        return Ok(Some((chunk?, (events, file))));
                    }
                    file = None;
                }
                let frame = if heartbeat.is_empty() {
                    events.next().await
                } else {
                    match tokio::time::timeout(Duration::from_secs(15), events.next()).await {
                        Ok(frame) => frame,
                        Err(_) => return Ok(Some((Bytes::from_static(heartbeat), (events, file)))),
                    }
                };
                match frame {
                    Some(Ok(WireEvent::Bytes(bytes))) => return Ok(Some((bytes, (events, file)))),
                    Some(Ok(WireEvent::File(spool))) => {
                        file = Some(super::file_stream(spool)?.boxed())
                    }
                    Some(Err(never)) => match never {},
                    None => return Ok::<_, io::Error>(None),
                }
            }
        },
    );
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/event-stream"),
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        axum::body::Body::from_stream(chunks),
    )
        .into_response()
}

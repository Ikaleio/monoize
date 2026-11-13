//! Request-scoped storage for wire strings that exceed the in-memory JSON threshold.
//!
//! Protocol codecs still use strings and serde_json values. They can move and
//! concatenate file references without loading the bytes. Only a reference
//! registered in the current request can resolve to a file.

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod json;
pub(crate) mod sse;

pub(crate) use json::{JsonInput, ReadError, read_json, write_json};

pub(crate) const CHUNK: usize = 65_536;
pub(crate) const WIRE_LIMIT: usize = if usize::BITS >= 64 {
    (10u64 * 1024 * 1024 * 1024) as usize
} else {
    usize::MAX
};
const PREFIX: &str = "~monoize-spool:";
const ID_LEN: usize = 64;

tokio::task_local! {
    static PAYLOAD: Arc<Payload>;
}

#[derive(Default)]
pub(crate) struct Payload {
    strings: Mutex<HashMap<String, Arc<Spool>>>,
}

pub(crate) fn current() -> Option<Arc<Payload>> {
    PAYLOAD.try_with(Arc::clone).ok()
}

pub(crate) async fn scoped<T>(payload: Arc<Payload>, work: impl Future<Output = T>) -> T {
    PAYLOAD.scope(payload, work).await
}

pub(crate) fn spawn<F>(work: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match current() {
        Some(payload) => tokio::spawn(scoped(payload, work)),
        None => tokio::spawn(work),
    }
}

static SPOOL_BYTES: AtomicU64 = AtomicU64::new(0);

fn spool_limit() -> u64 {
    static LIMIT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("MONOIZE_PAYLOAD_SPOOL_MAX_BYTES")
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
            .filter(|limit| *limit > 0)
            .unwrap_or(64 * 1024 * 1024 * 1024)
    })
}

pub(crate) struct Spool {
    path: tempfile::TempPath,
    len: u64,
    digest: [u8; 32],
}

impl Spool {
    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    pub(crate) fn open(&self) -> io::Result<File> {
        File::open(&self.path)
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        SPOOL_BYTES.fetch_sub(self.len, Ordering::Relaxed);
    }
}

struct OpenSpool {
    file: File,
    digest: Option<ring::digest::Context>,
    spool: Spool,
}

pub(crate) struct SpoolWriter(BufWriter<OpenSpool>);

impl SpoolWriter {
    pub(crate) fn new() -> io::Result<Self> {
        Self::create(true)
    }

    pub(crate) fn output() -> io::Result<Self> {
        Self::create(false)
    }

    fn create(hash: bool) -> io::Result<Self> {
        let file = match std::env::var_os("MONOIZE_PAYLOAD_SPOOL_DIR") {
            Some(path) => tempfile::NamedTempFile::new_in(path)?,
            None => tempfile::NamedTempFile::new()?,
        };
        let (file, path) = file.into_parts();
        Ok(Self(BufWriter::with_capacity(
            CHUNK,
            OpenSpool {
                file,
                digest: hash.then(|| ring::digest::Context::new(&ring::digest::SHA256)),
                spool: Spool {
                    path,
                    len: 0,
                    digest: [0; 32],
                },
            },
        )))
    }

    pub(crate) fn finish(mut self) -> io::Result<Spool> {
        self.0.flush()?;
        self.0
            .into_inner()
            .map(|mut open| {
                if let Some(digest) = open.digest {
                    open.spool.digest.copy_from_slice(digest.finish().as_ref());
                }
                open.spool
            })
            .map_err(|error| error.into_error())
    }
}

impl Write for SpoolWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Write for OpenSpool {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let requested = buf.len() as u64;
        SPOOL_BYTES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(requested)
                    .filter(|total| *total <= spool_limit())
            })
            .map_err(|_| io::Error::other("payload spool quota exceeded"))?;
        let result = self.file.write(buf);
        let written = result.as_ref().copied().unwrap_or(0) as u64;
        if let Some(digest) = &mut self.digest {
            digest.update(&buf[..written as usize]);
        }
        self.spool.len += written;
        SPOOL_BYTES.fetch_sub(requested - written, Ordering::Relaxed);
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Payload {
    pub(crate) fn insert(&self, spool: Spool) -> String {
        let marker = format!("{PREFIX}{}~", hex::encode(spool.digest));
        self.strings
            .lock()
            .unwrap()
            .entry(marker.clone())
            .or_insert_with(|| Arc::new(spool));
        marker
    }

    pub(crate) fn parts<'a, 'b>(&'b self, mut text: &'a str) -> impl Iterator<Item = Part<'a>> + 'b
    where
        'a: 'b,
    {
        std::iter::from_fn(move || {
            if text.is_empty() {
                return None;
            }
            let mut candidates = text.match_indices(PREFIX);
            for (offset, _) in &mut candidates {
                let end = offset + PREFIX.len() + ID_LEN + 1;
                let Some(marker) = text.get(offset..end) else {
                    continue;
                };
                if !marker.ends_with('~') {
                    continue;
                }
                let Some(spool) = self.strings.lock().unwrap().get(marker).cloned() else {
                    continue;
                };
                if offset > 0 {
                    let (prefix, rest) = text.split_at(offset);
                    text = rest;
                    return Some(Part::Inline(prefix));
                }
                text = &text[end..];
                return Some(Part::Disk(spool));
            }
            Some(Part::Inline(std::mem::take(&mut text)))
        })
    }
}

pub(crate) enum Part<'a> {
    Inline(&'a str),
    Disk(Arc<Spool>),
}

pub(crate) fn has_large_text(text: &str) -> bool {
    text.contains(PREFIX)
        && current().is_some_and(|payload| {
            payload
                .parts(text)
                .any(|part| matches!(part, Part::Disk(_)))
        })
}

pub(crate) fn text_len(text: &str) -> usize {
    if !text.contains(PREFIX) {
        return text.len();
    }
    current().map_or(text.len(), |payload| {
        payload
            .parts(text)
            .map(|part| match part {
                Part::Inline(text) => text.len(),
                Part::Disk(spool) => spool.len() as usize,
            })
            .sum()
    })
}

pub(crate) fn text_equal(left: &str, right: &str) -> Result<bool, String> {
    if left == right {
        return Ok(true);
    }
    if !(has_large_text(left) || has_large_text(right)) || text_len(left) != text_len(right) {
        return Ok(false);
    }
    let compare = || -> io::Result<bool> {
        let mut left = io::BufReader::with_capacity(CHUNK, text_reader(left)?);
        let mut right = io::BufReader::with_capacity(CHUNK, text_reader(right)?);
        use io::BufRead;
        loop {
            let a = left.fill_buf()?;
            let b = right.fill_buf()?;
            let n = a.len().min(b.len());
            if n == 0 {
                return Ok(a.is_empty() && b.is_empty());
            }
            if a[..n] != b[..n] {
                return Ok(false);
            }
            left.consume(n);
            right.consume(n);
        }
    };
    compare().map_err(|error| format!("read payload spool: {error}"))
}

pub(crate) fn redact_large(value: &mut Value) -> usize {
    match value {
        Value::String(text) if has_large_text(text) => {
            let len = text_len(text);
            *value = serde_json::json!({"omitted_bytes":len,"truncated":true});
            len
        }
        Value::Array(items) => items.iter_mut().map(redact_large).sum(),
        Value::Object(fields) => fields.values_mut().map(redact_large).sum(),
        _ => 0,
    }
}

pub(crate) fn contains_large(value: &Value) -> bool {
    match value {
        Value::String(text) => has_large_text(text),
        Value::Array(items) => items.iter().any(contains_large),
        Value::Object(fields) => fields.values().any(contains_large),
        _ => false,
    }
}

pub(crate) fn text_reader(text: &str) -> io::Result<Box<dyn Read + Send + '_>> {
    let Some(payload) = current() else {
        return Ok(Box::new(text.as_bytes()));
    };
    Ok(Box::new(TextReader {
        parts: payload.parts(text).collect::<Vec<_>>().into_iter(),
        active: None,
    }))
}

struct TextReader<'a> {
    parts: std::vec::IntoIter<Part<'a>>,
    active: Option<Box<dyn Read + Send + 'a>>,
}

impl Read for TextReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(active) = &mut self.active {
                let n = active.read(out)?;
                if n != 0 {
                    return Ok(n);
                }
            }
            self.active = match self.parts.next() {
                Some(Part::Inline(text)) => Some(Box::new(text.as_bytes())),
                Some(Part::Disk(spool)) => Some(Box::new(spool.open()?)),
                None => return Ok(0),
            };
        }
    }
}

pub(crate) fn file_stream(
    spool: Arc<Spool>,
) -> io::Result<impl Stream<Item = io::Result<Bytes>> + Send + 'static> {
    let file = tokio::fs::File::from_std(spool.open()?);
    Ok(
        tokio_util::io::ReaderStream::with_capacity(file, CHUNK).map(move |chunk| {
            let _spool = &spool;
            chunk
        }),
    )
}

pub(crate) fn json_stream(value: Value) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    let payload = current();
    blocking_stream(move |writer| write_json(writer, &value, payload.as_deref()))
}

fn blocking_stream(
    write: impl FnOnce(&mut BufWriter<ChunkWriter>) -> io::Result<()> + Send + 'static,
) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    tokio::task::spawn_blocking(move || {
        let mut writer = BufWriter::with_capacity(CHUNK, ChunkWriter(tx.clone()));
        let result = write(&mut writer).and_then(|()| writer.flush());
        if let Err(error) = result {
            let _ = tx.blocking_send(Err(error));
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

struct ChunkWriter(tokio::sync::mpsc::Sender<io::Result<Bytes>>);

impl Write for ChunkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = bytes.len().min(CHUNK);
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(&bytes[..n])))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn json_response(value: Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !contains_large(&value) {
        return axum::Json(value).into_response();
    }
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        axum::body::Body::from_stream(json_stream(value)),
    )
        .into_response()
}

pub(crate) fn http_limit(large: bool) -> usize {
    std::env::var("MONOIZE_HTTP_BODY_MAX_BYTES")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(if large { WIRE_LIMIT } else { 50 * 1024 * 1024 })
}

pub(crate) fn large_route(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::POST
        && matches!(
            path.strip_prefix("/api").unwrap_or(path),
            "/v1/responses"
                | "/v1/codex/responses"
                | "/v1/chat/completions"
                | "/v1/messages"
                | "/v1/images/generations"
                | "/v1/images/edits"
        )
}

pub(crate) struct JsonBody(pub(crate) Value);

impl<S: Send + Sync> axum::extract::FromRequest<S> for JsonBody {
    type Rejection = crate::error::AppError;

    async fn from_request(
        request: axum::extract::Request,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        use crate::error::AppError;
        use axum::http::{StatusCode, header};
        if !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<mime::Mime>().ok())
            .is_some_and(|mime| {
                mime.type_() == mime::APPLICATION
                    && (mime.subtype() == mime::JSON || mime.suffix() == Some(mime::JSON))
            })
        {
            return Err(AppError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_json",
                "expected a JSON content type",
            ));
        }
        let payload = current().ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "missing_payload_scope",
                "payload storage is unavailable",
            )
        })?;
        let limit = http_limit(true);
        let stream = request.into_body().into_data_stream().map(|chunk| {
            chunk.map_err(|error| {
                use std::error::Error;
                let mut cause = error.source();
                while let Some(source) = cause {
                    if source.is::<http_body_util::LengthLimitError>() {
                        return ReadError::Limit(limit);
                    }
                    cause = source.source();
                }
                ReadError::Transport(error.to_string())
            })
        });
        read_json(stream, payload, limit)
            .await
            .map(Self)
            .map_err(|error| {
                let (status, code) = match error {
                    ReadError::Limit(_) => (StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"),
                    ReadError::Spool(_) => {
                        (StatusCode::INSUFFICIENT_STORAGE, "payload_spool_failed")
                    }
                    _ => (StatusCode::BAD_REQUEST, "invalid_json"),
                };
                AppError::new(status, code, error.to_string())
            })
    }
}

pub(crate) fn spool_utf8(mut reader: impl Read) -> io::Result<String> {
    let payload = current().ok_or_else(|| io::Error::other("missing payload scope"))?;
    let mut out = SpoolWriter::new()?;
    let mut chunk = vec![0; CHUNK + 3];
    let mut carry = 0;
    loop {
        let n = reader.read(&mut chunk[carry..])?;
        if n == 0 {
            if carry != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated UTF-8",
                ));
            }
            return Ok(payload.insert(out.finish()?));
        }
        let len = carry + n;
        let valid = match std::str::from_utf8(&chunk[..len]) {
            Ok(_) => len,
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        };
        out.write_all(&chunk[..valid])?;
        chunk.copy_within(valid..len, 0);
        carry = len - valid;
    }
}

pub(crate) fn base64_stream(
    text: String,
) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    let payload = current().expect("large Base64 text belongs to a payload scope");
    blocking_stream(move |writer| {
        let parts = payload.parts(&text).collect::<Vec<_>>().into_iter();
        let reader = TextReader {
            parts,
            active: None,
        };
        let mut reader =
            base64::read::DecoderReader::new(reader, &base64::engine::general_purpose::STANDARD);
        io::copy(&mut reader, writer)?;
        Ok(())
    })
}

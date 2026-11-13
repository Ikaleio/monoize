use super::{CHUNK, Payload, SpoolWriter};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::io::{self, Write};
use std::sync::Arc;

const STRUCTURE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("body exceeds {0} bytes")]
    Limit(usize),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Invalid(String),
    #[error("payload spool: {0}")]
    Spool(#[from] io::Error),
}

pub(crate) async fn read_json(
    stream: impl Stream<Item = Result<Bytes, ReadError>>,
    payload: Arc<Payload>,
    limit: usize,
) -> Result<Value, ReadError> {
    let mut input = JsonInput::new(payload);
    let mut bytes = 0usize;
    tokio::pin!(stream);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes = bytes.saturating_add(chunk.len());
        if bytes > limit {
            return Err(ReadError::Limit(limit));
        }
        input.push(&chunk)?;
    }
    input.finish()
}

pub(crate) struct JsonInput {
    payload: Arc<Payload>,
    compact: Vec<u8>,
    string: Option<JsonString>,
    // Object member names are the only quoted tokens followed by a colon.
    pending_string: Option<JsonString>,
}

impl JsonInput {
    pub(crate) fn new(payload: Arc<Payload>) -> Self {
        Self {
            payload,
            compact: Vec::new(),
            string: None,
            pending_string: None,
        }
    }

    pub(crate) fn push(&mut self, mut bytes: &[u8]) -> Result<(), ReadError> {
        while !bytes.is_empty() {
            if let Some(string) = &mut self.string {
                let consumed = string.push(bytes)?;
                bytes = &bytes[consumed..];
                if string.closed {
                    self.pending_string = self.string.take();
                }
                continue;
            }
            if let Some(string) = self.pending_string.take() {
                let offset = bytes
                    .iter()
                    .position(|b| !matches!(b, b' ' | b'\n' | b'\r' | b'\t'));
                let Some(offset) = offset else {
                    self.pending_string = Some(string);
                    break;
                };
                bytes = &bytes[offset..];
                self.append_string(string, bytes[0] == b':')?;
            }
            let next = memchr::memchr(b'"', bytes).unwrap_or(bytes.len());
            self.append(&bytes[..next])?;
            if next == bytes.len() {
                break;
            }
            self.string = Some(JsonString::default());
            bytes = &bytes[next + 1..];
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), ReadError> {
        if self.compact.len().saturating_add(bytes.len()) > STRUCTURE_LIMIT {
            return Err(ReadError::Invalid(format!(
                "JSON structure exceeds {STRUCTURE_LIMIT} bytes"
            )));
        }
        self.compact.extend_from_slice(bytes);
        Ok(())
    }

    fn append_string(&mut self, string: JsonString, key: bool) -> Result<(), ReadError> {
        let text = string.finish(&self.payload, key)?;
        let encoded =
            serde_json::to_vec(&text).map_err(|error| ReadError::Invalid(error.to_string()))?;
        self.append(&encoded)
    }

    pub(crate) fn finish(mut self) -> Result<Value, ReadError> {
        if self.string.is_some() {
            return Err(ReadError::Invalid("unterminated JSON string".into()));
        }
        if let Some(string) = self.pending_string.take() {
            self.append_string(string, false)?;
        }
        serde_json::from_slice(&self.compact).map_err(|error| ReadError::Invalid(error.to_string()))
    }
}

#[derive(Default)]
struct JsonString {
    inline: Vec<u8>,
    spool: Option<SpoolWriter>,
    prefix: String,
    utf8_tail: Vec<u8>,
    escape: Vec<u8>,
    closed: bool,
}

impl JsonString {
    fn push(&mut self, bytes: &[u8]) -> Result<usize, ReadError> {
        let mut pos = 0;
        while pos < bytes.len() {
            if !self.escape.is_empty() {
                self.escape.push(bytes[pos]);
                pos += 1;
                let len = match self.escape.as_slice() {
                    [b'\\', b'u', ..] if self.escape.len() < 6 => continue,
                    [b'\\', b'u', first, second, ..]
                        if matches!(first, b'd' | b'D')
                            && matches!(second, b'8'..=b'9' | b'a'..=b'b' | b'A'..=b'B')
                            && self.escape.len() < 12 =>
                    {
                        continue;
                    }
                    _ => self.escape.len(),
                };
                let mut token = [b'"'; 14];
                token[1..len + 1].copy_from_slice(&self.escape);
                let text: String = serde_json::from_slice(&token[..len + 2])
                    .map_err(|error| ReadError::Invalid(error.to_string()))?;
                self.escape.clear();
                self.write(text.as_bytes())?;
                continue;
            }
            let remaining = &bytes[pos..];
            let next = memchr::memchr2(b'"', b'\\', remaining).unwrap_or(remaining.len());
            self.raw(&remaining[..next])?;
            pos += next;
            if pos == bytes.len() {
                break;
            }
            if !self.utf8_tail.is_empty() {
                return Err(ReadError::Invalid("incomplete UTF-8 in JSON string".into()));
            }
            let closed = bytes[pos] == b'"';
            pos += 1;
            if closed {
                self.closed = true;
                break;
            }
            self.escape.push(b'\\');
        }
        Ok(pos)
    }

    fn raw(&mut self, mut bytes: &[u8]) -> Result<(), ReadError> {
        if bytes.iter().any(|&b| b < 0x20) {
            return Err(ReadError::Invalid(
                "unescaped control character in JSON string".into(),
            ));
        }
        if !self.utf8_tail.is_empty() {
            while let Some((&byte, rest)) = bytes.split_first() {
                self.utf8_tail.push(byte);
                bytes = rest;
                match std::str::from_utf8(&self.utf8_tail) {
                    Ok(_) => {
                        let tail = std::mem::take(&mut self.utf8_tail);
                        self.write(&tail)?;
                        break;
                    }
                    Err(error) if error.error_len().is_none() => continue,
                    Err(_) => {
                        return Err(ReadError::Invalid("invalid UTF-8 in JSON string".into()));
                    }
                }
            }
        }
        match std::str::from_utf8(bytes) {
            Ok(_) => self.write(bytes),
            Err(error) if error.error_len().is_none() => {
                let (head, tail) = bytes.split_at(error.valid_up_to());
                self.utf8_tail.extend_from_slice(tail);
                self.write(head)
            }
            Err(_) => Err(ReadError::Invalid("invalid UTF-8 in JSON string".into())),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), ReadError> {
        if let Some(spool) = &mut self.spool {
            return Ok(spool.write_all(bytes)?);
        }
        if self.inline.len() + bytes.len() <= CHUNK {
            self.inline.extend_from_slice(bytes);
            return Ok(());
        }
        let take = CHUNK - self.inline.len();
        self.inline.extend_from_slice(&bytes[..take]);
        let mut spool = SpoolWriter::new()?;
        // Keep a data URL's MIME header visible to existing media codecs.
        let prefix_len = if self.inline.starts_with(b"data:") {
            memchr::memchr(b',', &self.inline)
                .filter(|&end| self.inline[..end].ends_with(b";base64"))
                .and_then(|end| std::str::from_utf8(&self.inline[..=end]).ok())
                .map_or(0, |prefix| {
                    self.prefix = prefix.to_owned();
                    prefix.len()
                })
        } else {
            0
        };
        spool.write_all(&self.inline[prefix_len..])?;
        spool.write_all(&bytes[take..])?;
        self.inline = Vec::new();
        self.spool = Some(spool);
        Ok(())
    }

    fn finish(self, payload: &Payload, key: bool) -> Result<String, ReadError> {
        if let Some(spool) = self.spool {
            if key {
                return Err(ReadError::Invalid(format!(
                    "JSON key exceeds {CHUNK} bytes"
                )));
            }
            Ok(self.prefix + &payload.insert(spool.finish()?))
        } else {
            let text = String::from_utf8(self.inline).expect("JSON string was checked as UTF-8");
            if !key && text.contains(super::PREFIX) {
                // External strings cannot impersonate an earlier reference from the same request.
                let mut spool = SpoolWriter::new()?;
                spool.write_all(text.as_bytes())?;
                return Ok(payload.insert(spool.finish()?));
            }
            Ok(text)
        }
    }
}

pub(crate) fn write_json(
    out: &mut impl Write,
    value: &Value,
    payload: Option<&Payload>,
) -> io::Result<()> {
    match value {
        Value::String(text) => {
            out.write_all(b"\"")?;
            match payload {
                Some(payload) => {
                    for part in payload.parts(text) {
                        match part {
                            super::Part::Inline(text) => write_escaped(out, text.as_bytes())?,
                            super::Part::Disk(spool) => {
                                use std::io::Read;
                                let mut file = spool.open()?;
                                let mut chunk = vec![0; CHUNK];
                                loop {
                                    let n = file.read(&mut chunk)?;
                                    if n == 0 {
                                        break;
                                    }
                                    write_escaped(out, &chunk[..n])?;
                                }
                            }
                        }
                    }
                }
                None => write_escaped(out, text.as_bytes())?,
            }
            out.write_all(b"\"")
        }
        Value::Array(items) => {
            out.write_all(b"[")?;
            for (i, item) in items.iter().enumerate() {
                if i != 0 {
                    out.write_all(b",")?;
                }
                write_json(out, item, payload)?;
            }
            out.write_all(b"]")
        }
        Value::Object(fields) => {
            out.write_all(b"{")?;
            for (i, (key, value)) in fields.iter().enumerate() {
                if i != 0 {
                    out.write_all(b",")?;
                }
                serde_json::to_writer(&mut *out, key)?;
                out.write_all(b":")?;
                write_json(out, value, payload)?;
            }
            out.write_all(b"}")
        }
        _ => serde_json::to_writer(out, value).map_err(io::Error::other),
    }
}

fn write_escaped(out: &mut impl Write, mut bytes: &[u8]) -> io::Result<()> {
    while let Some(i) = bytes
        .iter()
        .position(|&b| matches!(b, b'"' | b'\\' | 0..=31))
    {
        out.write_all(&bytes[..i])?;
        let escaped: &[u8] = match bytes[i] {
            b'"' => br#"\""#,
            b'\\' => br#"\\"#,
            b'\n' => br#"\n"#,
            b'\r' => br#"\r"#,
            b'\t' => br#"\t"#,
            8 => br#"\b"#,
            12 => br#"\f"#,
            control => {
                write!(out, "\\u{control:04x}")?;
                bytes = &bytes[i + 1..];
                continue;
            }
        };
        out.write_all(escaped)?;
        bytes = &bytes[i + 1..];
    }
    out.write_all(bytes)
}

use super::{CHUNK, PREFIX, Payload, SpoolWriter, text_marker};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::collections::{HashMap, hash_map::Entry};
use std::io::{self, Write};
use std::sync::Arc;

const STRUCTURE_LIMIT: usize = 4 * 1024 * 1024;
const REQUEST_STRUCTURE_LIMIT: usize = 16 * 1024 * 1024;
const REQUEST_COMPACT_LIMIT: usize = 64 * 1024 * 1024;
const REQUEST_INLINE_TEXT_BUDGET: usize = 16 * 1024 * 1024;
const MIN_PACKED_TEXT: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("body exceeds {0} bytes")]
    Limit(usize),
    #[error("{component} exceeds {limit} bytes")]
    JsonLimit {
        component: &'static str,
        limit: usize,
    },
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
    JsonInput::new(payload).read(stream, limit).await
}

pub(crate) struct JsonInput {
    payload: Arc<Payload>,
    compact: Vec<u8>,
    structure_remaining: usize,
    request_text: Option<RequestText>,
    string: Option<JsonString>,
    // Object member names are the only quoted tokens followed by a colon.
    pending_string: Option<JsonString>,
}

impl JsonInput {
    pub(crate) fn new(payload: Arc<Payload>) -> Self {
        Self {
            payload,
            compact: Vec::new(),
            structure_remaining: STRUCTURE_LIMIT,
            request_text: None,
            string: None,
            pending_string: None,
        }
    }

    pub(super) fn for_request(payload: Arc<Payload>) -> Self {
        Self {
            structure_remaining: REQUEST_STRUCTURE_LIMIT,
            request_text: Some(RequestText::default()),
            ..Self::new(payload)
        }
    }

    pub(super) async fn read(
        mut self,
        stream: impl Stream<Item = Result<Bytes, ReadError>>,
        limit: usize,
    ) -> Result<Value, ReadError> {
        let mut bytes = 0usize;
        tokio::pin!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            bytes = bytes.saturating_add(chunk.len());
            if bytes > limit {
                return Err(ReadError::Limit(limit));
            }
            self.push(&chunk)?;
        }
        self.finish()
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
            self.append(&bytes[..next], next)?;
            if let Some(request_text) = &mut self.request_text {
                request_text.scan(&bytes[..next])?;
            }
            if next == bytes.len() {
                break;
            }
            self.string = Some(JsonString::default());
            bytes = &bytes[next + 1..];
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8], structure_len: usize) -> Result<(), ReadError> {
        let (structure_limit, compact_limit) = if self.request_text.is_some() {
            (REQUEST_STRUCTURE_LIMIT, REQUEST_COMPACT_LIMIT)
        } else {
            (STRUCTURE_LIMIT, STRUCTURE_LIMIT)
        };
        self.structure_remaining =
            self.structure_remaining
                .checked_sub(structure_len)
                .ok_or(ReadError::JsonLimit {
                    component: "JSON structure",
                    limit: structure_limit,
                })?;
        if self.compact.len().saturating_add(bytes.len()) > compact_limit {
            return Err(ReadError::JsonLimit {
                component: "In-memory JSON",
                limit: compact_limit,
            });
        }
        self.compact.extend_from_slice(bytes);
        Ok(())
    }

    fn append_string(&mut self, string: JsonString, key: bool) -> Result<(), ReadError> {
        let text = string.finish(&self.payload, key)?;
        let encoded = serde_json::to_vec(&text).map_err(invalid)?;
        let encoded = match &mut self.request_text {
            Some(request_text) if key => {
                request_text.key(&text);
                encoded
            }
            Some(request_text) if !text.contains(PREFIX) => request_text.pack(&text, encoded)?,
            _ => encoded,
        };
        // Value text has its own budget; its two quotes still contribute to the structure.
        let structure_len = if key { encoded.len() } else { 2 };
        self.append(&encoded, structure_len)
    }

    pub(crate) fn finish(mut self) -> Result<Value, ReadError> {
        if self.string.is_some() {
            return Err(ReadError::Invalid("unterminated JSON string".into()));
        }
        if let Some(string) = self.pending_string.take() {
            self.append_string(string, false)?;
        }
        let value = serde_json::from_slice(&self.compact).map_err(invalid)?;
        if let Some(request_text) = self.request_text {
            request_text.register(&self.payload)?;
        }
        Ok(value)
    }
}

fn invalid(error: serde_json::Error) -> ReadError {
    ReadError::Invalid(error.to_string())
}

#[derive(Default)]
struct RequestText {
    inline_bytes: usize,
    containers: Vec<Container>,
    spool: Option<SpoolWriter>,
    ranges: HashMap<String, (u64, u64)>,
    end: u64,
}

// Only protocol text positions accept new references. Another codec can
// parse a tool argument as nested JSON and needs its contents inline.
#[derive(Clone, Copy)]
enum Position {
    Root,
    History,
    Item,
    Content,
    Part,
    Text,
    Other,
}

enum Container {
    Object {
        position: Position,
        member: Position,
    },
    Array(Position),
}

impl RequestText {
    fn position(&self) -> Position {
        match self.containers.last() {
            Some(Container::Object { member, .. }) | Some(Container::Array(member)) => *member,
            None => Position::Root,
        }
    }

    fn key(&mut self, key: &str) {
        use Position::*;
        if let Some(Container::Object { position, member }) = self.containers.last_mut() {
            *member = match (*position, key) {
                (Root, "input" | "messages") => History,
                (Root, "instructions" | "system" | "prompt") => Content,
                (Item | Part, "content" | "output" | "summary") => Content,
                (Item | Part, "text" | "thinking") => Text,
                _ => Other,
            };
        }
    }

    fn scan(&mut self, bytes: &[u8]) -> Result<(), ReadError> {
        use Position::*;
        for byte in bytes {
            let container = match byte {
                b'{' => Container::Object {
                    position: self.position(),
                    member: Other,
                },
                b'[' => Container::Array(match self.position() {
                    History => Item,
                    Content => Part,
                    _ => Other,
                }),
                b'}' | b']' => {
                    self.containers.pop();
                    continue;
                }
                _ => continue,
            };
            if self.containers.len() == 127 {
                return Err(ReadError::Invalid("recursion limit exceeded".into()));
            }
            self.containers.push(container);
        }
        Ok(())
    }

    fn pack(&mut self, text: &str, encoded: Vec<u8>) -> Result<Vec<u8>, ReadError> {
        let new_inline_bytes = self.inline_bytes + encoded.len() - 2;
        let spillable = matches!(
            self.position(),
            Position::History
                | Position::Item
                | Position::Content
                | Position::Part
                | Position::Text
        );
        if spillable
            && new_inline_bytes > REQUEST_INLINE_TEXT_BUDGET
            && text.len() >= MIN_PACKED_TEXT
        {
            let prefix_len = data_url_header_len(text.as_bytes());
            if text.len() - prefix_len >= MIN_PACKED_TEXT {
                let marker = self.store(&text[prefix_len..])?;
                let packed = serde_json::to_vec(&(text[..prefix_len].to_owned() + &marker))
                    .map_err(invalid)?;
                self.inline_bytes += packed.len() - 2 - marker.len();
                return Ok(packed);
            }
        }
        self.inline_bytes = new_inline_bytes;
        Ok(encoded)
    }

    fn store(&mut self, text: &str) -> io::Result<String> {
        let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
        let marker = text_marker(digest.as_ref());
        if let Entry::Vacant(entry) = self.ranges.entry(marker.clone()) {
            let spool = match &mut self.spool {
                Some(spool) => spool,
                None => self.spool.insert(SpoolWriter::output()?),
            };
            spool.write_all(text.as_bytes())?;
            let len = text.len() as u64;
            entry.insert((self.end, len));
            self.end += len;
        }
        Ok(marker)
    }

    fn register(self, payload: &Payload) -> io::Result<()> {
        if let Some(spool) = self.spool {
            payload.insert_ranges(spool.finish()?, self.ranges);
        }
        Ok(())
    }
}

fn data_url_header_len(bytes: &[u8]) -> usize {
    if !bytes.starts_with(b"data:") {
        return 0;
    }
    memchr::memchr(b',', bytes)
        .filter(|&end| bytes[..end].ends_with(b";base64"))
        .and_then(|end| std::str::from_utf8(&bytes[..=end]).ok())
        .map_or(0, str::len)
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
        let prefix_len = data_url_header_len(&self.inline);
        self.prefix = std::str::from_utf8(&self.inline[..prefix_len])
            .expect("data URL header was checked as UTF-8")
            .to_owned();
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
            if !key && text.contains(PREFIX) {
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
                                let mut file = spool.reader_at(0)?;
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

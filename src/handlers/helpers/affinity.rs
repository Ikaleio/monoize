use super::*;
use canonical::canonical_affinity_node;
use std::io::Write;
use xxhash_rust::xxh3::Xxh3;

mod canonical;

fn affinity_value_from_json(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .as_i64()
                .map(|v| v.to_string())
                .or_else(|| value.as_u64().map(|v| v.to_string()))
        })
}

/// CM-AFF-1b: raw conversation identifier from the decoded request body.
/// Returns the identifier string itself so a header uuid and a body uuid match.
pub(in crate::handlers) fn stable_session_affinity_raw(req: &urp::UrpRequest) -> Option<String> {
    const SESSION_KEYS: &[&str] = &[
        "session_id",
        "session",
        "conversation_id",
        "conversation",
        "thread_id",
        "thread",
    ];
    for key in SESSION_KEYS {
        if let Some(value) = req.extra_body.get(*key).and_then(affinity_value_from_json) {
            return Some(value);
        }
    }
    if let Some(metadata) = req.extra_body.get("metadata").and_then(Value::as_object) {
        for key in SESSION_KEYS {
            if let Some(value) = metadata.get(*key).and_then(affinity_value_from_json) {
                return Some(value);
            }
        }
    }
    if let Some(value) = req
        .extra_body
        .get("user_id")
        .and_then(affinity_value_from_json)
    {
        return Some(value);
    }
    if let Some(metadata) = req.extra_body.get("metadata").and_then(Value::as_object)
        && let Some(value) = metadata.get("user_id").and_then(affinity_value_from_json)
    {
        return Some(value);
    }
    req.user
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn stable_affinity_field(req: &urp::UrpRequest) -> Option<String> {
    if let Some(previous_response_id) = req
        .extra_body
        .get("previous_response_id")
        .and_then(affinity_value_from_json)
    {
        return Some(format!("previous_response_id:{previous_response_id}"));
    }
    if let Some(user) = req.user.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        return Some(format!("user:{user}"));
    }
    const KEYS: &[&str] = &[
        "session_id",
        "session",
        "conversation_id",
        "conversation",
        "thread_id",
        "thread",
        "user_id",
        "user",
    ];
    for key in KEYS {
        if let Some(value) = req.extra_body.get(*key).and_then(affinity_value_from_json) {
            return Some(format!("{key}:{value}"));
        }
    }
    if let Some(metadata) = req.extra_body.get("metadata").and_then(Value::as_object) {
        for key in KEYS {
            if let Some(value) = metadata.get(*key).and_then(affinity_value_from_json) {
                return Some(format!("metadata.{key}:{value}"));
            }
        }
    }
    None
}

pub(crate) fn short_xxh3_hex(input: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(input.as_bytes()))
}

const AFFINITY_PREFIX_NODE_LIMIT: usize = 8;
const AFFINITY_PREFIX_BYTE_LIMIT: usize = 16 * 1024;

struct BoundedHashWriter {
    hasher: Xxh3,
    remaining: usize,
    limit_reached: bool,
}

impl BoundedHashWriter {
    fn new(limit: usize) -> Self {
        Self {
            hasher: Xxh3::new(),
            remaining: limit,
            limit_reached: false,
        }
    }

    fn digest(&self) -> u64 {
        self.hasher.digest()
    }
}

impl Write for BoundedHashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let accepted = self.remaining.min(buf.len());
        if accepted > 0 {
            self.hasher.update(&buf[..accepted]);
            self.remaining -= accepted;
        }
        if accepted < buf.len() {
            self.limit_reached = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "affinity prefix byte limit reached",
            ));
        }
        Ok(accepted)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn affinity_prefix_hash(req: &urp::UrpRequest) -> String {
    let mut writer = BoundedHashWriter::new(AFFINITY_PREFIX_BYTE_LIMIT);
    let result = (|| -> std::io::Result<()> {
        writer.write_all(b"[")?;
        for (index, node) in req
            .input
            .iter()
            .take(AFFINITY_PREFIX_NODE_LIMIT)
            .enumerate()
        {
            if index > 0 {
                writer.write_all(b",")?;
            }
            if let Err(error) = serde_json::to_writer(&mut writer, &canonical_affinity_node(node)) {
                if writer.limit_reached {
                    return Ok(());
                }
                return Err(std::io::Error::other(error));
            }
        }
        writer.write_all(b"]")
    })();
    if result.is_err() && !writer.limit_reached {
        return short_xxh3_hex("");
    }
    format!("{:016x}", writer.digest())
}

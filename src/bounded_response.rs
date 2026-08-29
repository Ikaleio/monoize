use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;

pub(crate) const DEFAULT_UPSTREAM_DISCOVERY_MAX_BYTES: usize = 16_777_216;
const UPSTREAM_DISCOVERY_MAX_BYTES_ENV: &str = "MONOIZE_UPSTREAM_DISCOVERY_MAX_BYTES";

#[derive(Debug, thiserror::Error)]
pub(crate) enum BoundedResponseError {
    #[error(
        "upstream discovery response exceeds the {max_bytes}-byte limit: Content-Length is {content_length}"
    )]
    DeclaredLengthExceeded {
        content_length: u64,
        max_bytes: usize,
    },
    #[error(
        "upstream discovery response exceeds the {max_bytes}-byte limit while reading the body"
    )]
    StreamedLengthExceeded { max_bytes: usize },
    #[error("failed to read upstream discovery response body: {source}")]
    BodyRead {
        #[source]
        source: reqwest::Error,
    },
}

impl BoundedResponseError {
    pub(crate) fn is_limit_exceeded(&self) -> bool {
        matches!(
            self,
            Self::DeclaredLengthExceeded { .. } | Self::StreamedLengthExceeded { .. }
        )
    }
}

pub(crate) fn upstream_discovery_max_bytes() -> usize {
    upstream_discovery_max_bytes_from_raw(
        std::env::var(UPSTREAM_DISCOVERY_MAX_BYTES_ENV)
            .ok()
            .as_deref(),
    )
}

fn upstream_discovery_max_bytes_from_raw(raw: Option<&str>) -> usize {
    raw.map(str::trim)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_UPSTREAM_DISCOVERY_MAX_BYTES)
}

pub(crate) async fn read_upstream_discovery_body(
    response: reqwest::Response,
) -> Result<Bytes, BoundedResponseError> {
    read_response_body_with_limit(response, upstream_discovery_max_bytes()).await
}

async fn read_response_body_with_limit(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Bytes, BoundedResponseError> {
    let max_bytes_u64 = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    if let Some(content_length) = response.content_length()
        && content_length > max_bytes_u64
    {
        return Err(BoundedResponseError::DeclaredLengthExceeded {
            content_length,
            max_bytes,
        });
    }

    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| BoundedResponseError::BodyRead { source })?;
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(BoundedResponseError::StreamedLengthExceeded { max_bytes });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformError, TransformRuntimeContext,
    TransformScope, TransformState, UrpData,
};
use crate::urp::{ImageSource, Node, OrdinaryRole};
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_MAX_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default = "default_timeout_seconds")]
    timeout_seconds: u64,
    #[serde(default = "default_max_bytes")]
    max_bytes: usize,
    #[serde(default)]
    roles: Option<Vec<String>>,
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_max_bytes() -> usize {
    DEFAULT_MAX_BYTES
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ResolveImageUrlsTransform;

#[async_trait]
impl Transform for ResolveImageUrlsTransform {
    fn type_id(&self) -> &'static str {
        "resolve_image_urls"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[TransformScope::Provider, TransformScope::ApiKey]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "default": DEFAULT_TIMEOUT_SECONDS
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "default": DEFAULT_MAX_BYTES
                },
                "roles": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        Ok(Box::new(cfg))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(NoState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?
            .clone();
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        let allowed_roles = parse_allowed_roles(&cfg.roles);

        let mut futures = Vec::new();

        for (node_idx, node) in req.input.iter().enumerate() {
            let Some(role) = node.role() else {
                continue;
            };
            if !allowed_roles.contains(&role) {
                continue;
            }
            if let Node::Image {
                source: ImageSource::Url { url, .. },
                ..
            } = node
            {
                if is_data_url(url.as_str()) {
                    continue;
                }
                let client = context.http_client.clone();
                let url = url.clone();
                let timeout = std::time::Duration::from_secs(cfg.timeout_seconds);
                let max_bytes = cfg.max_bytes;
                futures.push((
                    node_idx,
                    tokio::spawn(async move {
                        fetch_image_as_base64(&client, &url, timeout, max_bytes).await
                    }),
                ));
            }
        }

        for (node_idx, handle) in futures {
            let result = handle
                .await
                .map_err(|e| TransformError::Apply(format!("image fetch task failed: {e}")))?;
            match result {
                Ok((media_type, b64_data)) => {
                    if let Some(Node::Image { source, .. }) = req.input.get_mut(node_idx) {
                        *source = ImageSource::Base64 {
                            media_type,
                            data: b64_data,
                        };
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        node_idx,
                        error = %e,
                        "resolve_image_urls: failed to fetch image, keeping original URL"
                    );
                }
            }
        }

        Ok(())
    }
}

fn parse_allowed_roles(roles: &Option<Vec<String>>) -> Vec<OrdinaryRole> {
    match roles {
        None => vec![
            OrdinaryRole::User,
            OrdinaryRole::Assistant,
            OrdinaryRole::System,
            OrdinaryRole::Developer,
        ],
        Some(names) => names
            .iter()
            .filter_map(|name| match name.as_str() {
                "user" => Some(OrdinaryRole::User),
                "assistant" => Some(OrdinaryRole::Assistant),
                "system" => Some(OrdinaryRole::System),
                "developer" => Some(OrdinaryRole::Developer),
                _ => None,
            })
            .collect(),
    }
}

fn is_data_url(url: &str) -> bool {
    url.starts_with("data:")
}

async fn fetch_image_as_base64(
    client: &reqwest::Client,
    url: &str,
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Result<(String, String), String> {
    let resp = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed for {url}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| infer_media_type_from_url(url));

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed to read body for {url}: {e}"))?;

    if bytes.len() > max_bytes {
        return Err(format!(
            "image too large ({} bytes, max {}): {url}",
            bytes.len(),
            max_bytes
        ));
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((content_type, b64))
}

fn infer_media_type_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        _ => "application/octet-stream",
    }
    .to_string()
}

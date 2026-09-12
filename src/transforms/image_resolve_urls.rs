use crate::transforms::{
    Phase, Transform, TransformConfig, TransformError, TransformRuntimeContext, TransformScope,
    TransformState, UrpData,
};
use crate::urp::{ImageSource, Node, OrdinaryRole, UrpStreamEvent};
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

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

pub struct ImageResolveUrlsTransform;

#[derive(Default)]
struct ResolveState {
    resolved: HashMap<String, ImageSource>,
}

impl TransformState for ResolveState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[async_trait]
impl Transform for ImageResolveUrlsTransform {
    fn type_id(&self) -> &'static str {
        "image_resolve_urls"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Image: resolve URLs to base64"),
            ("zh", "图像：URL 解析为 base64"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Downloads request or response image URLs and replaces them with inline base64 sources; failed fetches leave nodes unchanged.",
            ),
            (
                "zh",
                "下载请求或响应中的图片 URL 并替换为内联 base64；下载失败的节点保持不变。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request, Phase::Response]
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
        Box::new(ResolveState::default())
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?
            .clone();
        let state = state
            .as_any_mut()
            .downcast_mut::<ResolveState>()
            .ok_or_else(|| TransformError::Apply("invalid state type".to_string()))?;
        let nodes: &mut [Node] = match data {
            UrpData::Request(req) => &mut req.input,
            UrpData::Response(resp) => &mut resp.output,
            UrpData::Stream(UrpStreamEvent::NodeDone { node, .. }) => std::slice::from_mut(node),
            UrpData::Stream(UrpStreamEvent::ResponseDone { output, .. }) => output,
            _ => return Ok(()),
        };

        let allowed_roles = parse_allowed_roles(&cfg.roles);

        let mut futures = Vec::new();

        for (node_idx, node) in nodes.iter_mut().enumerate() {
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
                if let Some(resolved) = state.resolved.get(url) {
                    if let Node::Image { source, .. } = node {
                        *source = resolved.clone();
                    }
                    continue;
                }
                let client = context.http_client.clone();
                let url = url.clone();
                let timeout = std::time::Duration::from_secs(cfg.timeout_seconds);
                let max_bytes = cfg.max_bytes;
                futures.push((
                    node_idx,
                    url.clone(),
                    tokio::spawn(async move {
                        fetch_image_as_base64(&client, &url, timeout, max_bytes).await
                    }),
                ));
            }
        }

        for (node_idx, url, handle) in futures {
            let result = handle
                .await
                .map_err(|e| TransformError::Apply(format!("image fetch task failed: {e}")))?;
            match result {
                Ok((media_type, b64_data)) => {
                    let resolved = ImageSource::Base64 {
                        media_type,
                        data: b64_data,
                    };
                    state.resolved.insert(url, resolved.clone());
                    if let Some(Node::Image { source, .. }) = nodes.get_mut(node_idx) {
                        *source = resolved;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        node_idx,
                        error = %e,
                        "image_resolve_urls: failed to fetch image, keeping original URL"
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
    let mut resp = client
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

    if resp
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("image exceeds {max_bytes} bytes: {url}"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("failed to read body for {url}: {e}"))?
    {
        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
            return Err(format!("image exceeds {max_bytes} bytes: {url}"));
        }
        bytes.extend_from_slice(&chunk);
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

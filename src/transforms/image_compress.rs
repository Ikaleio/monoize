use crate::image_transform_cache::CachedImagePayload;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{ImageSource, Node, NodeDelta, NodeHeader, OrdinaryRole, UrpStreamEvent};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageEncoder};
use mozjpeg::{ColorSpace, Compress};
use oxipng::Options;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::any::Any;
use std::collections::HashMap;
use std::io::Cursor;

const TRANSFORM_VERSION: &str = "compress_user_message_images:v5";

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default)]
    max_edge_px: Option<u32>,
    #[serde(default = "default_jpeg_quality")]
    jpeg_quality: u8,
    #[serde(default = "default_jpegxl_quality")]
    jpegxl_quality: u8,
    #[serde(default = "default_jpegxl_effort")]
    jpegxl_effort: u8,
    #[serde(default = "default_webp_quality")]
    webp_quality: u8,
    #[serde(default = "default_skip_if_smaller")]
    skip_if_smaller: bool,
    #[serde(default)]
    output_format: OutputFormat,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum OutputFormat {
    #[default]
    Original,
    Jpg,
    #[serde(rename = "jpegxl_lossless")]
    JpegxlLossless,
    Jpegxl,
    #[serde(rename = "webp_lossless")]
    WebpLossless,
    Webp,
    Png,
}

impl OutputFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Jpg => "jpg",
            Self::JpegxlLossless => "jpegxl_lossless",
            Self::Jpegxl => "jpegxl",
            Self::WebpLossless => "webp_lossless",
            Self::Webp => "webp",
            Self::Png => "png",
        }
    }
}

fn default_jpeg_quality() -> u8 {
    80
}

fn default_jpegxl_quality() -> u8 {
    90
}

fn default_jpegxl_effort() -> u8 {
    7
}

fn default_webp_quality() -> u8 {
    80
}

fn default_skip_if_smaller() -> bool {
    true
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ImageCompressInputTransform;

#[derive(Default)]
struct AssistantStreamState {
    assistant_image_nodes: HashMap<u32, bool>,
}

impl TransformState for AssistantStreamState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ImageCompressOutputTransform;

#[async_trait]
impl Transform for ImageCompressInputTransform {
    fn type_id(&self) -> &'static str {
        "image_compress_input"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[("en", "Image: compress request images"), ("zh", "图像：压缩请求图片")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Re-encodes and optionally resizes base64 user-message images in the request to reduce upstream payload size."),
            ("zh", "对请求中 user 消息的 base64 图片重新编码并可选缩放，以减小上游请求体积。"),
        ]
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
                "max_edge_px": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional maximum width or height of compressed user-message images. Omit to preserve the original dimensions."
                },
                "jpeg_quality": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 80,
                    "description": "JPEG quality used for JPEG output."
                },
                "jpegxl_quality": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 99,
                    "default": 90,
                    "description": "Quality used for lossy JPEG XL output. Quality 100 is reserved for the separate lossless mode."
                },
                "jpegxl_effort": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "default": 7,
                    "description": "JPEG XL encoding effort for both modes. Higher values compress more slowly."
                },
                "webp_quality": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 80,
                    "description": "Quality used for lossy WebP output."
                },
                "skip_if_smaller": {
                    "type": "boolean",
                    "default": true,
                    "description": "Keep the original image when compression does not reduce payload size."
                },
                "output_format": {
                    "type": "string",
                    "enum": ["original", "jpg", "jpegxl_lossless", "jpegxl", "webp_lossless", "webp", "png"],
                    "default": "original",
                    "description": "Output image format. Original preserves the source format."
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        if cfg.max_edge_px == Some(0) {
            return Err(TransformError::InvalidConfig(
                "max_edge_px must be >= 1".to_string(),
            ));
        }
        if !(1..=100).contains(&cfg.jpeg_quality) {
            return Err(TransformError::InvalidConfig(
                "jpeg_quality must be between 1 and 100".to_string(),
            ));
        }
        if !(1..=99).contains(&cfg.jpegxl_quality) {
            return Err(TransformError::InvalidConfig(
                "jpegxl_quality must be between 1 and 99".to_string(),
            ));
        }
        if !(1..=10).contains(&cfg.jpegxl_effort) {
            return Err(TransformError::InvalidConfig(
                "jpegxl_effort must be between 1 and 10".to_string(),
            ));
        }
        if !(1..=100).contains(&cfg.webp_quality) {
            return Err(TransformError::InvalidConfig(
                "webp_quality must be between 1 and 100".to_string(),
            ));
        }
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

        compress_image_nodes(&mut req.input, OrdinaryRole::User, context, &cfg).await?;

        Ok(())
    }
}

#[async_trait]
impl Transform for ImageCompressOutputTransform {
    fn type_id(&self) -> &'static str {
        "image_compress_output"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[("en", "Image: compress response images"), ("zh", "图像：压缩响应图片")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Re-encodes and optionally resizes assistant output images in the response or stream."),
            ("zh", "对响应或流中 assistant 输出的图片重新编码并可选缩放。"),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[TransformScope::Provider, TransformScope::ApiKey]
    }

    fn config_schema(&self) -> Value {
        image_compression_config_schema("assistant-output")
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        parse_image_compression_config(raw)
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(AssistantStreamState::default())
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

        match data {
            UrpData::Response(resp) => {
                compress_image_nodes(&mut resp.output, OrdinaryRole::Assistant, context, &cfg)
                    .await?;
            }
            UrpData::Stream(event) => {
                let Some(stream_state) = state.as_any_mut().downcast_mut::<AssistantStreamState>()
                else {
                    return Err(TransformError::Apply("invalid stream state".to_string()));
                };
                compress_stream_event(event, stream_state, context, &cfg).await?;
            }
            UrpData::Request(_) => {}
        }

        Ok(())
    }
}

fn image_compression_config_schema(subject: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "max_edge_px": {
                "type": "integer",
                "minimum": 1,
                "description": format!("Optional maximum width or height of compressed {subject} images. Omit to preserve the original dimensions.")
            },
            "jpeg_quality": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100,
                "default": 80,
                "description": "JPEG quality used for JPEG output."
            },
            "jpegxl_quality": {
                "type": "integer",
                "minimum": 1,
                "maximum": 99,
                "default": 90,
                "description": "Quality used for lossy JPEG XL output. Quality 100 is reserved for the separate lossless mode."
            },
            "jpegxl_effort": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10,
                "default": 7,
                "description": "JPEG XL encoding effort for both modes. Higher values compress more slowly."
            },
            "webp_quality": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100,
                "default": 80,
                "description": "Quality used for lossy WebP output."
            },
            "skip_if_smaller": {
                "type": "boolean",
                "default": true,
                "description": "Keep the original image when compression does not reduce payload size."
            },
            "output_format": {
                "type": "string",
                "enum": ["original", "jpg", "jpegxl_lossless", "jpegxl", "webp_lossless", "webp", "png"],
                "default": "original",
                "description": "Output image format. Original preserves the source format."
            }
        },
        "additionalProperties": false
    })
}

fn parse_image_compression_config(raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
    let cfg: Config =
        serde_json::from_value(raw).map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
    if cfg.max_edge_px == Some(0) {
        return Err(TransformError::InvalidConfig(
            "max_edge_px must be >= 1".to_string(),
        ));
    }
    if !(1..=100).contains(&cfg.jpeg_quality) {
        return Err(TransformError::InvalidConfig(
            "jpeg_quality must be between 1 and 100".to_string(),
        ));
    }
    if !(1..=99).contains(&cfg.jpegxl_quality) {
        return Err(TransformError::InvalidConfig(
            "jpegxl_quality must be between 1 and 99".to_string(),
        ));
    }
    if !(1..=10).contains(&cfg.jpegxl_effort) {
        return Err(TransformError::InvalidConfig(
            "jpegxl_effort must be between 1 and 10".to_string(),
        ));
    }
    if !(1..=100).contains(&cfg.webp_quality) {
        return Err(TransformError::InvalidConfig(
            "webp_quality must be between 1 and 100".to_string(),
        ));
    }
    Ok(Box::new(cfg))
}

fn split_image_data_url(url: &str) -> Option<(String, String)> {
    let payload = url.strip_prefix("data:")?;
    let (meta, data) = payload.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    let media_type = meta.trim_end_matches(";base64");
    if !media_type.starts_with("image/") || media_type.is_empty() || data.is_empty() {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

async fn compress_image_nodes(
    nodes: &mut [Node],
    role: OrdinaryRole,
    context: &TransformRuntimeContext,
    cfg: &Config,
) -> Result<(), TransformError> {
    for node in nodes {
        compress_image_node(node, role, context, cfg).await?;
    }
    Ok(())
}

async fn compress_image_node(
    node: &mut Node,
    role: OrdinaryRole,
    context: &TransformRuntimeContext,
    cfg: &Config,
) -> Result<(), TransformError> {
    let Node::Image {
        role: node_role,
        source,
        ..
    } = node
    else {
        return Ok(());
    };
    if *node_role != role {
        return Ok(());
    }
    if let Some(next_source) = compress_image_source(context, cfg, source).await? {
        *source = next_source;
    }
    Ok(())
}

async fn compress_image_source(
    context: &TransformRuntimeContext,
    cfg: &Config,
    source: &ImageSource,
) -> Result<Option<ImageSource>, TransformError> {
    match source {
        ImageSource::Base64 { media_type, data } => {
            compress_base64_image(context, cfg.clone(), media_type.clone(), data.clone()).await
        }
        ImageSource::Url { url, detail } => {
            let Some((media_type, data)) = split_image_data_url(url.as_str()) else {
                return Ok(None);
            };
            let Some(next_source) =
                compress_base64_image(context, cfg.clone(), media_type, data).await?
            else {
                return Ok(None);
            };
            Ok(Some(preserve_url_detail(next_source, detail.clone())))
        }
        ImageSource::FileId { .. } => Ok(None),
    }
}

async fn compress_stream_event(
    event: &mut UrpStreamEvent,
    stream_state: &mut AssistantStreamState,
    context: &TransformRuntimeContext,
    cfg: &Config,
) -> Result<(), TransformError> {
    match event {
        UrpStreamEvent::NodeStart {
            node_index, header, ..
        } => {
            let is_assistant_image = matches!(
                header,
                NodeHeader::Image {
                    role: OrdinaryRole::Assistant,
                    ..
                }
            );
            stream_state
                .assistant_image_nodes
                .insert(*node_index, is_assistant_image);
        }
        UrpStreamEvent::NodeDelta {
            node_index, delta, ..
        } => {
            if stream_state
                .assistant_image_nodes
                .get(&*node_index)
                .copied()
                .unwrap_or(false)
            {
                if let NodeDelta::Image { source } = delta {
                    if let Some(next_source) = compress_image_source(context, cfg, source).await? {
                        *source = next_source;
                    }
                }
            }
        }
        UrpStreamEvent::NodeDone {
            node_index, node, ..
        } => {
            compress_image_node(node, OrdinaryRole::Assistant, context, cfg).await?;
            stream_state.assistant_image_nodes.remove(&*node_index);
        }
        UrpStreamEvent::ResponseDone { output, .. } => {
            compress_image_nodes(output, OrdinaryRole::Assistant, context, cfg).await?;
            stream_state.assistant_image_nodes.clear();
        }
        _ => {}
    }
    Ok(())
}

fn preserve_url_detail(source: ImageSource, detail: Option<String>) -> ImageSource {
    match source {
        ImageSource::Base64 { media_type, data } => ImageSource::Url {
            url: format!("data:{media_type};base64,{data}"),
            detail,
        },
        ImageSource::Url {
            url,
            detail: next_detail,
        } => ImageSource::Url {
            url,
            detail: next_detail.or(detail),
        },
        source @ ImageSource::FileId { .. } => source,
    }
}

async fn compress_base64_image(
    context: &TransformRuntimeContext,
    cfg: Config,
    media_type: String,
    base64_data: String,
) -> Result<Option<ImageSource>, TransformError> {
    if !is_supported_media_type(&media_type) {
        return Ok(None);
    }
    let limits = context.image_transform_cache.limits();
    let max_base64_bytes = limits
        .max_encoded_bytes
        .saturating_add(2)
        .saturating_div(3)
        .saturating_mul(4);
    if base64_data.len() > max_base64_bytes {
        return Ok(None);
    }
    let original = match STANDARD.decode(base64_data.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    if original.len() > limits.max_encoded_bytes {
        return Ok(None);
    }
    let cache_key = build_cache_key(&media_type, &cfg, &original);
    if let Some(hit) = context
        .image_transform_cache
        .read_if_fresh(&cache_key)
        .await
        .map_err(TransformError::Apply)?
    {
        return Ok(Some(ImageSource::Base64 {
            media_type: hit.media_type,
            data: hit.data_base64,
        }));
    }

    let original_len = original.len();
    let _permit = context
        .image_transform_cache
        .acquire_transform_permit()
        .await
        .map_err(TransformError::Apply)?;
    let media_type_for_task = media_type.clone();
    let cfg_for_task = cfg.clone();
    let original_for_task = original.clone();
    let max_pixels = limits.max_pixels;
    let transformed = tokio::task::spawn_blocking(move || {
        compress_image_bytes_with_limit(
            &media_type_for_task,
            &original_for_task,
            &cfg_for_task,
            max_pixels,
        )
    })
    .await
    .map_err(|err| TransformError::Apply(format!("image compression task join failed: {err}")))??;

    let Some(transformed) = transformed else {
        return Ok(None);
    };

    if cfg.skip_if_smaller && transformed.bytes.len() >= original_len {
        return Ok(None);
    }

    let payload = CachedImagePayload {
        media_type: transformed.media_type.clone(),
        data_base64: STANDARD.encode(&transformed.bytes),
    };
    if let Err(err) = context
        .image_transform_cache
        .write(&cache_key, &payload)
        .await
    {
        tracing::warn!("persist image transform cache entry failed: {err}");
    }
    Ok(Some(ImageSource::Base64 {
        media_type: payload.media_type,
        data: payload.data_base64,
    }))
}

fn is_supported_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/jpeg" | "image/jpg" | "image/png" | "image/webp"
    )
}

fn build_cache_key(media_type: &str, cfg: &Config, original: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(TRANSFORM_VERSION.as_bytes());
    digest.update([0]);
    digest.update(media_type.as_bytes());
    digest.update([0]);
    digest.update(cfg.max_edge_px.unwrap_or_default().to_le_bytes());
    digest.update([cfg.jpeg_quality]);
    digest.update([cfg.jpegxl_quality]);
    digest.update([cfg.jpegxl_effort]);
    digest.update([cfg.webp_quality]);
    digest.update([u8::from(cfg.skip_if_smaller)]);
    digest.update(cfg.output_format.as_str().as_bytes());
    digest.update([0]);
    digest.update(original);
    digest_hex(&digest.finalize())
}

struct CompressedImageBytes {
    media_type: String,
    bytes: Vec<u8>,
}

#[cfg(test)]
fn compress_image_bytes(
    media_type: &str,
    original: &[u8],
    cfg: &Config,
) -> Result<Option<CompressedImageBytes>, TransformError> {
    compress_image_bytes_with_limit(media_type, original, cfg, u64::MAX)
}

fn compress_image_bytes_with_limit(
    media_type: &str,
    original: &[u8],
    cfg: &Config,
    max_pixels: u64,
) -> Result<Option<CompressedImageBytes>, TransformError> {
    let reader = match image::ImageReader::new(Cursor::new(original)).with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => return Ok(None),
    };
    let dimensions = match reader.into_dimensions() {
        Ok(dimensions) => dimensions,
        Err(_) => return Ok(None),
    };
    if u64::from(dimensions.0).saturating_mul(u64::from(dimensions.1)) > max_pixels {
        return Ok(None);
    }
    let decoded = match image::load_from_memory(original) {
        Ok(image) => image,
        Err(_) => return Ok(None),
    };
    let resized = resize_if_needed(decoded, cfg.max_edge_px);
    let output_format = match cfg.output_format {
        OutputFormat::Original => output_format_for_media_type(media_type),
        selected => Some(selected),
    };
    let Some(output_format) = output_format else {
        return Ok(None);
    };

    let transformed = match output_format {
        OutputFormat::Original => unreachable!("original output format must be resolved"),
        OutputFormat::Jpg => CompressedImageBytes {
            media_type: "image/jpeg".to_string(),
            bytes: encode_image_as_jpeg(&resized, cfg.jpeg_quality)?,
        },
        OutputFormat::JpegxlLossless => CompressedImageBytes {
            media_type: "image/jxl".to_string(),
            bytes: encode_image_as_jpegxl(&resized, true, cfg.jpegxl_quality, cfg.jpegxl_effort)?,
        },
        OutputFormat::Jpegxl => CompressedImageBytes {
            media_type: "image/jxl".to_string(),
            bytes: encode_image_as_jpegxl(&resized, false, cfg.jpegxl_quality, cfg.jpegxl_effort)?,
        },
        OutputFormat::WebpLossless => CompressedImageBytes {
            media_type: "image/webp".to_string(),
            bytes: encode_image_as_webp_lossless(&resized)?,
        },
        OutputFormat::Webp => CompressedImageBytes {
            media_type: "image/webp".to_string(),
            bytes: encode_image_as_webp_lossy(&resized, cfg.webp_quality)?,
        },
        OutputFormat::Png => CompressedImageBytes {
            media_type: "image/png".to_string(),
            bytes: encode_image_as_png(&resized)?,
        },
    };
    Ok(Some(transformed))
}

#[cfg(test)]
fn sha256_hex(input: &[u8]) -> String {
    digest_hex(&Sha256::digest(input))
}

fn digest_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn output_format_for_media_type(media_type: &str) -> Option<OutputFormat> {
    match media_type {
        "image/jpeg" | "image/jpg" => Some(OutputFormat::Jpg),
        "image/png" => Some(OutputFormat::Png),
        "image/webp" => Some(OutputFormat::WebpLossless),
        _ => None,
    }
}

fn encode_image_as_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, TransformError> {
    let rgb = image.to_rgb8();
    encode_jpeg_with_mozjpeg(rgb.as_raw(), rgb.width(), rgb.height(), quality)
}

fn encode_image_as_png(image: &DynamicImage) -> Result<Vec<u8>, TransformError> {
    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, PngFilterType::Adaptive);
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        encoder
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|err| TransformError::Apply(format!("encode png: {err}")))?;
    } else {
        let rgb = image.to_rgb8();
        encoder
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|err| TransformError::Apply(format!("encode png: {err}")))?;
    }
    optimize_png_losslessly(&out)
}

fn encode_image_as_webp_lossless(image: &DynamicImage) -> Result<Vec<u8>, TransformError> {
    let mut out = Vec::new();
    let encoder = WebPEncoder::new_lossless(&mut out);
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        encoder
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|err| TransformError::Apply(format!("encode webp: {err}")))?;
    } else {
        let rgb = image.to_rgb8();
        encoder
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|err| TransformError::Apply(format!("encode webp: {err}")))?;
    }
    Ok(out)
}

fn encode_image_as_webp_lossy(
    image: &DynamicImage,
    quality: u8,
) -> Result<Vec<u8>, TransformError> {
    let result = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        let config = fast_lossy_webp_config(quality)?;
        webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
            .encode_advanced(&config)
    } else {
        let rgb = image.to_rgb8();
        let config = fast_lossy_webp_config(quality)?;
        webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height()).encode_advanced(&config)
    };
    result
        .map(|encoded| encoded.to_vec())
        .map_err(|err| TransformError::Apply(format!("encode lossy webp: {err:?}")))
}

fn fast_lossy_webp_config(quality: u8) -> Result<webp::WebPConfig, TransformError> {
    let mut config = webp::WebPConfig::new()
        .map_err(|_| TransformError::Apply("initialize libwebp config".to_string()))?;
    config.lossless = 0;
    config.quality = f32::from(quality);
    config.method = 0;
    config.thread_level = 1;
    Ok(config)
}

struct LibJxlEncoder(*mut jxl_sys::JxlEncoder);

struct LibJxlParallelRunner(*mut std::ffi::c_void);

impl LibJxlParallelRunner {
    fn new() -> Result<Self, TransformError> {
        // SAFETY: the query has no preconditions and a null memory manager requests the default
        // allocator for the runner.
        let worker_threads = unsafe { jxl_sys::JxlThreadParallelRunnerDefaultNumWorkerThreads() };
        let handle =
            unsafe { jxl_sys::JxlThreadParallelRunnerCreate(std::ptr::null(), worker_threads) };
        if handle.is_null() {
            return Err(TransformError::Apply(
                "create libjxl parallel runner: out of memory".to_string(),
            ));
        }
        Ok(Self(handle))
    }
}

impl Drop for LibJxlParallelRunner {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by JxlThreadParallelRunnerCreate and is destroyed once.
        unsafe { jxl_sys::JxlThreadParallelRunnerDestroy(self.0) };
    }
}

impl LibJxlEncoder {
    fn check(
        &self,
        status: jxl_sys::JxlEncoderStatus,
        operation: &str,
    ) -> Result<(), TransformError> {
        if status == jxl_sys::JxlEncoderStatus::JXL_ENC_SUCCESS {
            return Ok(());
        }
        // SAFETY: self owns a live encoder handle until Drop runs.
        let detail = unsafe { jxl_sys::JxlEncoderGetError(self.0) };
        Err(TransformError::Apply(format!(
            "{operation}: libjxl status {status:?}, error {detail:?}"
        )))
    }
}

impl Drop for LibJxlEncoder {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by JxlEncoderCreate and is destroyed exactly once.
        unsafe { jxl_sys::JxlEncoderDestroy(self.0) };
    }
}

fn encode_image_as_jpegxl(
    image: &DynamicImage,
    lossless: bool,
    quality: u8,
    effort: u8,
) -> Result<Vec<u8>, TransformError> {
    let has_alpha = image.color().has_alpha();
    let (pixels, channels) = if has_alpha {
        (image.to_rgba8().into_raw(), 4)
    } else {
        (image.to_rgb8().into_raw(), 3)
    };
    let (width, height) = image.dimensions();

    let runner = LibJxlParallelRunner::new()?;

    // SAFETY: a null memory manager requests libjxl's default allocator.
    let handle = unsafe { jxl_sys::JxlEncoderCreate(std::ptr::null()) };
    if handle.is_null() {
        return Err(TransformError::Apply(
            "create libjxl encoder: out of memory".to_string(),
        ));
    }
    let encoder = LibJxlEncoder(handle);

    // SAFETY: encoder and runner are live. The runner is declared before the encoder, so it
    // outlives the encoder and all encoding work.
    encoder.check(
        unsafe {
            jxl_sys::JxlEncoderSetParallelRunner(
                encoder.0,
                Some(jxl_sys::JxlThreadParallelRunner),
                runner.0,
            )
        },
        "set libjxl parallel runner",
    )?;

    let mut info = jxl_sys::JxlBasicInfo::default();
    // SAFETY: info points to valid writable storage initialized to a valid zero bit pattern.
    unsafe { jxl_sys::JxlEncoderInitBasicInfo(&mut info) };
    info.xsize = width;
    info.ysize = height;
    info.bits_per_sample = 8;
    info.exponent_bits_per_sample = 0;
    info.num_color_channels = 3;
    info.num_extra_channels = u32::from(has_alpha);
    info.alpha_bits = if has_alpha { 8 } else { 0 };
    info.alpha_exponent_bits = 0;
    info.alpha_premultiplied = 0;
    info.uses_original_profile = i32::from(lossless);
    // SAFETY: encoder and fully initialized info are valid; libjxl copies the value.
    encoder.check(
        unsafe { jxl_sys::JxlEncoderSetBasicInfo(encoder.0, &info) },
        "set libjxl basic info",
    )?;

    let mut color = jxl_sys::JxlColorEncoding::default();
    // SAFETY: color is writable and libjxl fully initializes it as nonlinear sRGB.
    unsafe { jxl_sys::JxlColorEncodingSetToSRGB(&mut color, 0) };
    // SAFETY: encoder and color are valid; libjxl copies the value.
    encoder.check(
        unsafe { jxl_sys::JxlEncoderSetColorEncoding(encoder.0, &color) },
        "set libjxl color encoding",
    )?;

    // SAFETY: encoder is valid and a null source requests default frame settings.
    let frame_settings =
        unsafe { jxl_sys::JxlEncoderFrameSettingsCreate(encoder.0, std::ptr::null()) };
    if frame_settings.is_null() {
        return Err(TransformError::Apply(
            "create libjxl frame settings: out of memory".to_string(),
        ));
    }
    if lossless {
        // SAFETY: frame_settings belongs to the live encoder.
        encoder.check(
            unsafe { jxl_sys::JxlEncoderSetFrameLossless(frame_settings, 1) },
            "enable lossless libjxl encoding",
        )?;
    } else {
        // SAFETY: quality is validated in the documented range and frame_settings is live.
        let distance = unsafe { jxl_sys::JxlEncoderDistanceFromQuality(f32::from(quality)) };
        encoder.check(
            unsafe { jxl_sys::JxlEncoderSetFrameDistance(frame_settings, distance) },
            "set lossy libjxl quality",
        )?;
    }
    // SAFETY: config validation limits effort to libjxl's valid 1 through 10 range.
    encoder.check(
        unsafe {
            jxl_sys::JxlEncoderFrameSettingsSetOption(
                frame_settings,
                jxl_sys::JxlEncoderFrameSettingId::JXL_ENC_FRAME_SETTING_EFFORT,
                i64::from(effort),
            )
        },
        "set libjxl encoding effort",
    )?;

    let pixel_format = jxl_sys::JxlPixelFormat {
        num_channels: channels,
        data_type: jxl_sys::JxlDataType::JXL_TYPE_UINT8,
        endianness: jxl_sys::JxlEndianness::JXL_NATIVE_ENDIAN,
        align: 0,
    };
    // SAFETY: pixel_format describes the owned pixels buffer exactly; libjxl copies its contents.
    encoder.check(
        unsafe {
            jxl_sys::JxlEncoderAddImageFrame(
                frame_settings,
                &pixel_format,
                pixels.as_ptr().cast::<std::ffi::c_void>(),
                pixels.len(),
            )
        },
        "add libjxl image frame",
    )?;
    // SAFETY: no more input will be added to this live encoder.
    unsafe { jxl_sys::JxlEncoderCloseInput(encoder.0) };

    const INITIAL_CHUNK: usize = 65_536;
    const MAX_CHUNK: usize = 67_108_864;
    let mut output = Vec::new();
    let mut chunk = INITIAL_CHUNK;
    loop {
        let offset = output.len();
        output.resize(offset + chunk, 0);
        // SAFETY: resize established a writable region of chunk bytes starting at offset.
        let mut next_out = unsafe { output.as_mut_ptr().add(offset) };
        let mut available = chunk;
        // SAFETY: encoder is live and the output pointers describe the writable region above.
        let status =
            unsafe { jxl_sys::JxlEncoderProcessOutput(encoder.0, &mut next_out, &mut available) };
        output.truncate(offset + chunk - available);
        match status {
            jxl_sys::JxlEncoderStatus::JXL_ENC_SUCCESS => return Ok(output),
            jxl_sys::JxlEncoderStatus::JXL_ENC_NEED_MORE_OUTPUT => {
                chunk = chunk.saturating_mul(2).min(MAX_CHUNK);
            }
            _ => {
                // SAFETY: encoder is still live and owns the detailed error state.
                let detail = unsafe { jxl_sys::JxlEncoderGetError(encoder.0) };
                return Err(TransformError::Apply(format!(
                    "encode jpeg xl: libjxl status {status:?}, error {detail:?}"
                )));
            }
        }
    }
}

fn encode_jpeg_with_mozjpeg(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, TransformError> {
    std::panic::catch_unwind(|| {
        let mut compressor = Compress::new(ColorSpace::JCS_RGB);
        compressor.set_size(width as usize, height as usize);
        compressor.set_fastest_defaults();
        compressor.set_quality(quality as f32);
        let mut compressor = compressor
            .start_compress(Vec::new())
            .map_err(|err| TransformError::Apply(format!("start mozjpeg compression: {err}")))?;
        compressor
            .write_scanlines(rgb)
            .map_err(|err| TransformError::Apply(format!("encode jpeg with mozjpeg: {err}")))?;
        compressor
            .finish()
            .map_err(|err| TransformError::Apply(format!("finish mozjpeg compression: {err}")))
    })
    .map_err(|_| TransformError::Apply("mozjpeg panicked while encoding jpeg".to_string()))?
}

fn optimize_png_losslessly(bytes: &[u8]) -> Result<Vec<u8>, TransformError> {
    let mut options = Options::max_compression();
    options.strip = oxipng::StripChunks::Safe;
    oxipng::optimize_from_memory(bytes, &options)
        .map_err(|err| TransformError::Apply(format!("optimize png with oxipng: {err}")))
}

fn resize_if_needed(image: DynamicImage, max_edge_px: Option<u32>) -> DynamicImage {
    let Some(max_edge_px) = max_edge_px else {
        return image;
    };
    let (width, height) = image.dimensions();
    if width.max(height) <= max_edge_px {
        return image;
    }
    image.resize(max_edge_px, max_edge_px, FilterType::Lanczos3)
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ImageCompressInputTransform),
});

inventory::submit!(TransformEntry {
    factory: || Box::new(ImageCompressOutputTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::{TransformRuntimeContext, build_states_for_rules, registry};
    use crate::urp::internal_legacy_bridge::{Item, Part, Role, items_to_nodes, nodes_to_items};
    use crate::urp::{NodeHeader, UrpRequest, UrpResponse, UrpStreamEvent};
    use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
    use image::{ImageBuffer, ImageEncoder, Rgb};
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn compression_defaults_preserve_dimensions_and_format() {
        let cfg: Config = serde_json::from_value(json!({})).expect("default config");

        assert_eq!(cfg.max_edge_px, None);
        assert_eq!(cfg.output_format, OutputFormat::Original);
        assert_eq!(cfg.jpeg_quality, 80);
        assert_eq!(cfg.jpegxl_quality, 90);
        assert_eq!(cfg.jpegxl_effort, 7);
        assert_eq!(cfg.webp_quality, 80);
        assert!(cfg.skip_if_smaller);
    }

    #[test]
    fn image_cache_key_uses_sha256_digest_material() {
        let cfg = Config {
            max_edge_px: None,
            jpeg_quality: 80,
            jpegxl_quality: 90,
            jpegxl_effort: 7,
            webp_quality: 80,
            skip_if_smaller: true,
            output_format: OutputFormat::Original,
        };
        let key = build_cache_key("image/png", &cfg, b"original bytes");
        let same = build_cache_key("image/png", &cfg, b"original bytes");
        let changed_config = build_cache_key(
            "image/png",
            &Config {
                jpeg_quality: 81,
                ..cfg.clone()
            },
            b"original bytes",
        );
        let changed_original = build_cache_key("image/png", &cfg, b"different bytes");
        let changed_jpegxl_effort = build_cache_key(
            "image/png",
            &Config {
                jpegxl_effort: 8,
                ..cfg.clone()
            },
            b"original bytes",
        );
        let changed_output_format = build_cache_key(
            "image/png",
            &Config {
                output_format: OutputFormat::Webp,
                ..cfg.clone()
            },
            b"original bytes",
        );

        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!key.chars().any(|c| c.is_ascii_uppercase()));
        assert_eq!(key, same);
        assert_ne!(key, changed_config);
        assert_ne!(key, changed_original);
        assert_ne!(key, changed_jpegxl_effort);
        assert_ne!(key, changed_output_format);
    }

    #[test]
    fn sha256_matches_standard_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pixel_limit_is_checked_before_full_decode() {
        let original = STANDARD.decode(build_png_base64(64, 48)).unwrap();
        let cfg: Config = serde_json::from_value(json!({})).unwrap();
        assert!(
            compress_image_bytes_with_limit("image/png", &original, &cfg, 64 * 48 - 1)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn encodes_explicit_jpegxl_modes_with_libjxl() {
        let original = STANDARD
            .decode(build_png_base64(64, 48))
            .expect("decode png fixture");
        for output_format in [OutputFormat::JpegxlLossless, OutputFormat::Jpegxl] {
            let transformed = compress_image_bytes(
                "image/png",
                &original,
                &Config {
                    max_edge_px: None,
                    jpeg_quality: 80,
                    jpegxl_quality: 90,
                    jpegxl_effort: 7,
                    webp_quality: 80,
                    skip_if_smaller: false,
                    output_format,
                },
            )
            .expect("compress image")
            .expect("supported image");

            assert_eq!(transformed.media_type, "image/jxl");
            assert_eq!(transformed.bytes.get(..2), Some([0xff, 0x0a].as_slice()));
        }
    }

    #[test]
    fn encodes_explicit_lossy_webp_output_with_libwebp() {
        let original = STANDARD
            .decode(build_png_base64(64, 48))
            .expect("decode png fixture");
        let transformed = compress_image_bytes(
            "image/png",
            &original,
            &Config {
                max_edge_px: None,
                jpeg_quality: 80,
                jpegxl_quality: 90,
                jpegxl_effort: 7,
                webp_quality: 100,
                skip_if_smaller: false,
                output_format: OutputFormat::Webp,
            },
        )
        .expect("compress image")
        .expect("supported image");

        assert_eq!(transformed.media_type, "image/webp");
        assert_eq!(transformed.bytes.get(..4), Some(b"RIFF".as_slice()));
        assert_eq!(
            image::load_from_memory(&transformed.bytes)
                .expect("decode lossy webp")
                .dimensions(),
            (64, 48)
        );
    }

    #[test]
    fn jpeg_quality_changes_mozjpeg_output() {
        let original = STANDARD
            .decode(build_png_base64(64, 48))
            .expect("decode png fixture");
        let cfg = Config {
            max_edge_px: None,
            jpeg_quality: 30,
            jpegxl_quality: 90,
            jpegxl_effort: 7,
            webp_quality: 80,
            skip_if_smaller: false,
            output_format: OutputFormat::Jpg,
        };
        let low_quality = compress_image_bytes("image/png", &original, &cfg)
            .expect("compress low-quality jpeg")
            .expect("supported image");
        let high_quality = compress_image_bytes(
            "image/png",
            &original,
            &Config {
                jpeg_quality: 90,
                ..cfg
            },
        )
        .expect("compress high-quality jpeg")
        .expect("supported image");

        assert_ne!(low_quality.bytes, high_quality.bytes);
        assert!(low_quality.bytes.len() < high_quality.bytes.len());
    }

    #[test]
    #[ignore = "manual benchmark; set MONOIZE_BENCH_IMAGE"]
    fn benchmark_image_formats() {
        let path = std::env::var("MONOIZE_BENCH_IMAGE").expect("MONOIZE_BENCH_IMAGE");
        let selected_format = std::env::var("MONOIZE_BENCH_FORMAT").ok();
        let read_u8 = |name: &str, default: u8| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let jpeg_quality = read_u8("MONOIZE_BENCH_JPEG_QUALITY", 100);
        let jpegxl_quality = read_u8("MONOIZE_BENCH_JPEGXL_QUALITY", 99);
        let jpegxl_effort = read_u8("MONOIZE_BENCH_JPEGXL_EFFORT", 7);
        let webp_quality = read_u8("MONOIZE_BENCH_WEBP_QUALITY", 100);
        let original = std::fs::read(&path).expect("read benchmark image");
        println!(
            "source\tpath={path}\tbytes={}\tjpeg_quality={jpeg_quality}\tjpegxl_quality={jpegxl_quality}\tjpegxl_effort={jpegxl_effort}\twebp_quality={webp_quality}",
            original.len()
        );

        for (label, output_format) in [
            ("jpg", OutputFormat::Jpg),
            ("jpegxl_lossless", OutputFormat::JpegxlLossless),
            ("jpegxl", OutputFormat::Jpegxl),
            ("webp_lossless", OutputFormat::WebpLossless),
            ("webp", OutputFormat::Webp),
            ("png", OutputFormat::Png),
        ] {
            if selected_format
                .as_deref()
                .is_some_and(|selected| selected != label)
            {
                continue;
            }
            let started = std::time::Instant::now();
            let transformed = compress_image_bytes(
                "image/png",
                &original,
                &Config {
                    max_edge_px: None,
                    jpeg_quality,
                    jpegxl_quality,
                    jpegxl_effort,
                    webp_quality,
                    skip_if_smaller: false,
                    output_format,
                },
            )
            .expect("compress benchmark image")
            .expect("supported benchmark image");
            let ratio = transformed.bytes.len() as f64 / original.len() as f64;
            println!(
                "{label}\tbytes={}\tratio={ratio:.6}\tsaved={:.2}%\telapsed_ms={}",
                transformed.bytes.len(),
                (1.0 - ratio) * 100.0,
                started.elapsed().as_millis()
            );
        }
    }

    #[tokio::test]
    async fn compresses_user_message_base64_images_and_persists_cache() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(3600),
        )
        .await
        .expect("cache");
        let context = TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: None,
        };
        let input_png = build_png_base64(2048, 128);
        let mut req = UrpRequest {
            model: "gpt-test".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: input_png.clone(),
                    },
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "image_compress_input".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Request,
            config: json!({"skip_if_smaller": false}),
        }];
        let registry = registry();
        let mut states = build_states_for_rules(&rules, &registry).expect("states");

        crate::transforms::apply_transforms(
            UrpData::Request(&mut req),
            &rules,
            &mut states,
            "gpt-test",
            Phase::Request,
            &context,
            &registry,
        )
        .await
        .expect("apply transforms");

        let inputs = nodes_to_items(&req.input);
        let Item::Message { parts, .. } = &inputs[0] else {
            panic!("expected message item");
        };
        let Part::Image { source, .. } = &parts[0] else {
            panic!("expected image part");
        };
        let ImageSource::Base64 { media_type, data } = source else {
            panic!("expected base64 image source");
        };
        assert_eq!(media_type, "image/png");
        let compressed = STANDARD
            .decode(data.as_bytes())
            .expect("decode transformed image");
        let original = STANDARD
            .decode(input_png.as_bytes())
            .expect("decode original image");
        assert!(compressed.len() < original.len());
        assert_eq!(
            image::load_from_memory(&compressed)
                .expect("decode compressed image")
                .dimensions(),
            (2048, 128)
        );

        let entries = std::fs::read_dir(context.image_transform_cache.root())
            .expect("cache dir entries")
            .collect::<Result<Vec<_>, _>>()
            .expect("cache dir read");
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn compresses_user_message_data_url_images_and_preserves_detail() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(3600),
        )
        .await
        .expect("cache");
        let context = TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: None,
        };
        let input_png = build_png_data_url_source();
        let input_data_url = format!("data:image/png;base64,{input_png}");
        let mut req = UrpRequest {
            model: "gpt-test".to_string(),
            input: items_to_nodes(vec![Item::Message {
                id: None,
                role: Role::User,
                parts: vec![Part::Image {
                    source: ImageSource::Url {
                        url: input_data_url,
                        detail: Some("high".to_string()),
                    },
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            }]),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "image_compress_input".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Request,
            config: json!({
                "max_edge_px": 256,
                "jpeg_quality": 65,
                "output_format": "jpg"
            }),
        }];
        let registry = registry();
        let mut states = build_states_for_rules(&rules, &registry).expect("states");

        crate::transforms::apply_transforms(
            UrpData::Request(&mut req),
            &rules,
            &mut states,
            "gpt-test",
            Phase::Request,
            &context,
            &registry,
        )
        .await
        .expect("apply transforms");

        let inputs = nodes_to_items(&req.input);
        let Item::Message { parts, .. } = &inputs[0] else {
            panic!("expected message item");
        };
        let Part::Image { source, .. } = &parts[0] else {
            panic!("expected image part");
        };
        let ImageSource::Url { url, detail } = source else {
            panic!("expected data-url image source");
        };
        assert_eq!(detail.as_deref(), Some("high"));
        let Some((media_type, data)) = split_image_data_url(url.as_str()) else {
            panic!("expected transformed data url");
        };
        assert_eq!(media_type, "image/jpeg");
        let compressed = STANDARD
            .decode(data.as_bytes())
            .expect("decode transformed image");
        let original = STANDARD
            .decode(input_png.as_bytes())
            .expect("decode original image");
        assert!(compressed.len() < original.len());
        assert_eq!(
            image::load_from_memory(&compressed)
                .expect("decode compressed image")
                .dimensions(),
            (256, 192)
        );
    }

    #[tokio::test]
    async fn compresses_assistant_output_base64_images() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(3600),
        )
        .await
        .expect("cache");
        let context = TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: None,
        };
        let input_png = build_png_base64(2048, 128);
        let mut resp = UrpResponse {
            id: "resp-test".to_string(),
            model: "gpt-test".to_string(),
            created_at: None,
            output: vec![Node::Image {
                id: None,
                role: OrdinaryRole::Assistant,
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: input_png.clone(),
                },
                extra_body: HashMap::new(),
            }],
            finish_reason: None,
            usage: None,
            extra_body: HashMap::new(),
        };
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "image_compress_output".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Response,
            config: json!({"skip_if_smaller": false}),
        }];
        let registry = registry();
        let mut states = build_states_for_rules(&rules, &registry).expect("states");

        crate::transforms::apply_transforms(
            UrpData::Response(&mut resp),
            &rules,
            &mut states,
            "gpt-test",
            Phase::Response,
            &context,
            &registry,
        )
        .await
        .expect("apply transforms");

        let Node::Image { source, .. } = &resp.output[0] else {
            panic!("expected image node");
        };
        let ImageSource::Base64 { media_type, data } = source else {
            panic!("expected base64 image source");
        };
        assert_eq!(media_type, "image/png");
        let compressed = STANDARD
            .decode(data.as_bytes())
            .expect("decode transformed image");
        let original = STANDARD
            .decode(input_png.as_bytes())
            .expect("decode original image");
        assert!(compressed.len() < original.len());
        assert_eq!(
            image::load_from_memory(&compressed)
                .expect("decode compressed image")
                .dimensions(),
            (2048, 128)
        );
    }

    #[tokio::test]
    async fn compresses_assistant_image_stream_delta_after_assistant_image_start() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(3600),
        )
        .await
        .expect("cache");
        let context = TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: None,
        };
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "image_compress_output".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Response,
            config: json!({
                "max_edge_px": 256,
                "jpeg_quality": 65,
                "output_format": "jpg"
            }),
        }];
        let registry = registry();
        let mut states = build_states_for_rules(&rules, &registry).expect("states");
        let start = UrpStreamEvent::NodeStart {
            node_index: 7,
            header: NodeHeader::Image {
                id: None,
                role: OrdinaryRole::Assistant,
            },
            extra_body: HashMap::new(),
        };

        crate::transforms::apply_stream_transforms(
            start,
            &rules,
            &mut states,
            "gpt-test",
            Phase::Response,
            &context,
            &registry,
        )
        .await
        .expect("apply start transform");

        let input_png = build_png_data_url_source();
        let delta = UrpStreamEvent::NodeDelta {
            node_index: 7,
            delta: NodeDelta::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: input_png,
                },
            },
            usage: None,
            extra_body: HashMap::new(),
        };

        let mut transformed = crate::transforms::apply_stream_transforms(
            delta,
            &rules,
            &mut states,
            "gpt-test",
            Phase::Response,
            &context,
            &registry,
        )
        .await
        .expect("apply delta transform");

        let UrpStreamEvent::NodeDelta {
            delta: NodeDelta::Image { source },
            ..
        } = transformed.pop().expect("transformed delta")
        else {
            panic!("expected image delta");
        };
        let ImageSource::Base64 { media_type, .. } = source else {
            panic!("expected base64 image source");
        };
        assert_eq!(media_type, "image/jpeg");
    }

    fn build_png_data_url_source() -> String {
        build_png_base64(512, 384)
    }

    fn build_png_base64(width: u32, height: u32) -> String {
        let image = ImageBuffer::from_fn(width, height, |x, y| {
            let r = ((x * 13 + y * 3) % 255) as u8;
            let g = ((x * 7 + y * 11) % 255) as u8;
            let b = ((x * 17 + y * 5) % 255) as u8;
            Rgb([r, g, b])
        });
        let mut bytes = Vec::new();
        let encoder = PngEncoder::new_with_quality(
            &mut bytes,
            CompressionType::Fast,
            PngFilterType::Adaptive,
        );
        encoder
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode input png");
        STANDARD.encode(bytes)
    }
}

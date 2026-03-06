use crate::image_transform_cache::CachedImagePayload;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformState, UrpData,
};
use crate::urp::{ImageSource, Part, Role};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageEncoder};
use mozjpeg::{ColorSpace, Compress, ScanMode};
use oxipng::Options;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::any::Any;

const TRANSFORM_VERSION: &str = "compress_user_message_images:v2";

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default = "default_max_edge_px")]
    max_edge_px: u32,
    #[serde(default = "default_jpeg_quality")]
    jpeg_quality: u8,
    #[serde(default = "default_skip_if_smaller")]
    skip_if_smaller: bool,
}

fn default_max_edge_px() -> u32 {
    1568
}

fn default_jpeg_quality() -> u8 {
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

pub struct CompressUserMessageImagesTransform;

#[async_trait]
impl Transform for CompressUserMessageImagesTransform {
    fn type_id(&self) -> &'static str {
        "compress_user_message_images"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_edge_px": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum width or height of compressed user-message images. Defaults to 1568."
                },
                "jpeg_quality": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "JPEG quality used when the output image has no alpha channel. Defaults to 80."
                },
                "skip_if_smaller": {
                    "type": "boolean",
                    "description": "Keep the original image when compression does not reduce payload size. Defaults to true."
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config =
            serde_json::from_value(raw).map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        if cfg.max_edge_px == 0 {
            return Err(TransformError::InvalidConfig(
                "max_edge_px must be >= 1".to_string(),
            ));
        }
        if !(1..=100).contains(&cfg.jpeg_quality) {
            return Err(TransformError::InvalidConfig(
                "jpeg_quality must be between 1 and 100".to_string(),
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

        for message in &mut req.messages {
            if message.role != Role::User {
                continue;
            }
            for part in &mut message.parts {
                let Part::Image { source, .. } = part else {
                    continue;
                };
                let ImageSource::Base64 { media_type, data } = source else {
                    continue;
                };
                let Some(next_source) = compress_base64_image(
                    context,
                    cfg.clone(),
                    media_type.clone(),
                    data.clone(),
                )
                .await?
                else {
                    continue;
                };
                *source = next_source;
            }
        }

        Ok(())
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
    let original = match STANDARD.decode(base64_data.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
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
    let media_type_for_task = media_type.clone();
    let cfg_for_task = cfg.clone();
    let original_for_task = original.clone();
    let transformed = tokio::task::spawn_blocking(move || {
        compress_image_bytes(&media_type_for_task, &original_for_task, &cfg_for_task)
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
    if let Err(err) = context.image_transform_cache.write(&cache_key, &payload).await {
        tracing::warn!("persist image transform cache entry failed: {err}");
    }
    Ok(Some(ImageSource::Base64 {
        media_type: payload.media_type,
        data: payload.data_base64,
    }))
}

fn is_supported_media_type(media_type: &str) -> bool {
    matches!(media_type, "image/jpeg" | "image/jpg" | "image/png" | "image/webp")
}

fn build_cache_key(media_type: &str, cfg: &Config, original: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(TRANSFORM_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(cfg.max_edge_px.to_le_bytes());
    hasher.update([cfg.jpeg_quality]);
    hasher.update([u8::from(cfg.skip_if_smaller)]);
    hasher.update([0]);
    hasher.update(original);
    hex::encode(hasher.finalize())
}

struct CompressedImageBytes {
    media_type: String,
    bytes: Vec<u8>,
}

fn compress_image_bytes(
    _media_type: &str,
    original: &[u8],
    cfg: &Config,
) -> Result<Option<CompressedImageBytes>, TransformError> {
    let decoded = match image::load_from_memory(original) {
        Ok(image) => image,
        Err(_) => return Ok(None),
    };
    let resized = resize_if_needed(decoded, cfg.max_edge_px);
    if resized.color().has_alpha() {
        let rgba = resized.to_rgba8();
        let (width, height) = rgba.dimensions();
        let mut out = Vec::new();
        let encoder = PngEncoder::new_with_quality(
            &mut out,
            CompressionType::Best,
            PngFilterType::Adaptive,
        );
        encoder
            .write_image(rgba.as_raw(), width, height, image::ExtendedColorType::Rgba8)
            .map_err(|err| TransformError::Apply(format!("encode png: {err}")))?;
        let out = optimize_png_losslessly(&out)?;
        return Ok(Some(CompressedImageBytes {
            media_type: "image/png".to_string(),
            bytes: out,
        }));
    }

    let rgb = resized.to_rgb8();
    let out = encode_jpeg_with_mozjpeg(rgb.as_raw(), rgb.width(), rgb.height(), cfg.jpeg_quality)?;
    Ok(Some(CompressedImageBytes {
        media_type: "image/jpeg".to_string(),
        bytes: out,
    }))
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
        compressor.set_quality(quality as f32);
        compressor.set_progressive_mode();
        compressor.set_scan_optimization_mode(ScanMode::AllComponentsTogether);
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

fn resize_if_needed(image: DynamicImage, max_edge_px: u32) -> DynamicImage {
    let (width, height) = image.dimensions();
    if width.max(height) <= max_edge_px {
        return image;
    }
    image.resize(max_edge_px, max_edge_px, FilterType::Lanczos3)
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CompressUserMessageImagesTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::{TransformRuntimeContext, build_states_for_rules, registry};
    use crate::urp::{Message, UrpRequest};
    use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
    use image::{ImageBuffer, ImageEncoder, Rgb};
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[tokio::test]
    async fn compresses_user_message_base64_images_and_persists_cache() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(temp_dir.path().join("cache"), std::time::Duration::from_secs(3600))
            .await
            .expect("cache");
        let context = TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
        };
        let input_png = build_png_data_url_source();
        let mut req = UrpRequest {
            model: "gpt-test".to_string(),
            messages: vec![Message {
                role: Role::User,
                parts: vec![Part::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: input_png.clone(),
                    },
                    extra_body: HashMap::new(),
                }],
                extra_body: HashMap::new(),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        };
        let rules = vec![crate::transforms::TransformRuleConfig {
            transform: "compress_user_message_images".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Request,
            config: json!({"max_edge_px": 256, "jpeg_quality": 65}),
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

        let Part::Image { source, .. } = &req.messages[0].parts[0] else {
            panic!("expected image part");
        };
        let ImageSource::Base64 { media_type, data } = source else {
            panic!("expected base64 image source");
        };
        assert_eq!(media_type, "image/jpeg");
        let compressed = STANDARD.decode(data.as_bytes()).expect("decode transformed image");
        let original = STANDARD.decode(input_png.as_bytes()).expect("decode original image");
        assert!(compressed.len() < original.len());

        let entries = std::fs::read_dir(context.image_transform_cache.root())
            .expect("cache dir entries")
            .collect::<Result<Vec<_>, _>>()
            .expect("cache dir read");
        assert_eq!(entries.len(), 1);
    }

    fn build_png_data_url_source() -> String {
        let image = ImageBuffer::from_fn(512, 384, |x, y| {
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

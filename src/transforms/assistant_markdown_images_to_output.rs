use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData, response_output_items_mut,
};
use crate::urp::{ImageSource, Item, Part, Role, UrpStreamEvent};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct AssistantMarkdownImagesToOutputTransform;

#[async_trait]
impl Transform for AssistantMarkdownImagesToOutputTransform {
    fn type_id(&self) -> &'static str {
        "assistant_markdown_images_to_output"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Response]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[TransformScope::Provider, TransformScope::ApiKey]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config =
            serde_json::from_value(raw).map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        Ok(Box::new(cfg))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(NoState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        match data {
            UrpData::Response(resp) => {
                for item in response_output_items_mut(resp) {
                    rewrite_assistant_markdown_images(item);
                }
            }
            UrpData::Stream(event) => match event {
                UrpStreamEvent::ItemDone { item, .. } => rewrite_assistant_markdown_images(item),
                UrpStreamEvent::ResponseDone { outputs, .. } => {
                    for item in outputs {
                        rewrite_assistant_markdown_images(item);
                    }
                }
                _ => {}
            },
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn rewrite_assistant_markdown_images(item: &mut Item) {
    let Item::Message { role, parts, .. } = item else {
        return;
    };
    if *role != Role::Assistant {
        return;
    }
    let mut next_parts = Vec::with_capacity(parts.len());
    for part in parts.iter() {
        match part {
            Part::Text {
                content,
                extra_body,
            } => {
                let (cleaned, images) = extract_markdown_images_from_text(content);
                if !cleaned.is_empty() {
                    next_parts.push(Part::Text {
                        content: cleaned,
                        extra_body: extra_body.clone(),
                    });
                }
                next_parts.extend(images);
            }
            other => next_parts.push(other.clone()),
        }
    }
    *parts = next_parts;
}

fn markdown_image_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\(([^)\s]+)\)").expect("markdown image regex"))
}

fn extract_markdown_images_from_text(content: &str) -> (String, Vec<Part>) {
    let mut images = Vec::new();
    let mut cleaned = String::new();
    let mut last_end = 0usize;
    for caps in markdown_image_regex().captures_iter(content) {
        let Some(full) = caps.get(0) else {
            continue;
        };
        let Some(url_match) = caps.get(1) else {
            continue;
        };
        let Some(source) = parse_markdown_image_source(url_match.as_str()) else {
            continue;
        };
        cleaned.push_str(&content[last_end..full.start()]);
        images.push(Part::Image {
            source,
            extra_body: HashMap::new(),
        });
        last_end = full.end();
    }
    cleaned.push_str(&content[last_end..]);
    (cleaned, images)
}

fn parse_markdown_image_source(url: &str) -> Option<ImageSource> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        if !meta.ends_with(";base64") {
            return None;
        }
        let media_type = meta.trim_end_matches(";base64");
        if !media_type.starts_with("image/") || data.is_empty() {
            return None;
        }
        return Some(ImageSource::Base64 {
            media_type: media_type.to_string(),
            data: data.to_string(),
        });
    }
    Some(ImageSource::Url {
        url: url.to_string(),
        detail: None,
    })
}

inventory::submit!(TransformEntry {
    factory: || Box::new(AssistantMarkdownImagesToOutputTransform),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_url_and_base64_markdown_images() {
        let (cleaned, images) = extract_markdown_images_from_text(
            "hello ![a](https://example.com/a.png) world ![b](data:image/png;base64,QUJD)",
        );
        assert_eq!(cleaned, "hello  world ");
        assert_eq!(images.len(), 2);
        match &images[0] {
            Part::Image {
                source: ImageSource::Url { url, .. },
                ..
            } => assert_eq!(url, "https://example.com/a.png"),
            _ => panic!("expected url image"),
        }
        match &images[1] {
            Part::Image {
                source: ImageSource::Base64 { media_type, data },
                ..
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "QUJD");
            }
            _ => panic!("expected base64 image"),
        }
    }

}

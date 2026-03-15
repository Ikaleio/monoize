use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData, response_output_items_mut,
};
use crate::urp::{ImageSource, Item, Part, Role, UrpStreamEvent};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default)]
    template: Option<String>,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct AssistantOutputImagesToMarkdownTransform;

#[async_trait]
impl Transform for AssistantOutputImagesToMarkdownTransform {
    fn type_id(&self) -> &'static str {
        "assistant_output_images_to_markdown"
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
            "properties": {
                "template": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Template appended for each image. Supports {{src}}, {{url}}, {{media_type}}, and {{data}}."
                }
            },
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
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?
            .clone();
        match data {
            UrpData::Response(resp) => {
                for item in response_output_items_mut(resp) {
                    append_images_as_markdown(item, &cfg);
                }
            }
            UrpData::Stream(event) => match event {
                UrpStreamEvent::ItemDone { item, .. } => append_images_as_markdown(item, &cfg),
                UrpStreamEvent::ResponseDone { outputs, .. } => {
                    for item in outputs {
                        append_images_as_markdown(item, &cfg);
                    }
                }
                _ => {}
            },
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

fn append_images_as_markdown(item: &mut Item, config: &Config) {
    let Item::Message { role, parts, .. } = item else {
        return;
    };
    if *role != Role::Assistant {
        return;
    }
    let appended = parts
        .iter()
        .filter_map(|part| match part {
            Part::Image { source, .. } => Some(format_image_markdown(source, config)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if appended.is_empty() {
        return;
    }
    if let Some(Part::Text { content, .. }) = parts
        .iter_mut()
        .rev()
        .find(|part| matches!(part, Part::Text { .. }))
    {
        content.push_str(&appended);
        return;
    }
    parts.push(Part::Text {
        content: appended,
        extra_body: HashMap::new(),
    });
}

fn format_image_markdown(source: &ImageSource, config: &Config) -> String {
    let default = match source {
        ImageSource::Url { url, .. } => format!("![image]({url})"),
        ImageSource::Base64 { media_type, data } => {
            format!("![image](data:{media_type};base64,{data})")
        }
    };
    let Some(template) = config.template.as_deref() else {
        return default;
    };
    match source {
        ImageSource::Url { url, .. } => template
            .replace("{{src}}", url)
            .replace("{{url}}", url)
            .replace("{{media_type}}", "")
            .replace("{{data}}", ""),
        ImageSource::Base64 { media_type, data } => {
            let src = format!("data:{media_type};base64,{data}");
            template
                .replace("{{src}}", &src)
                .replace("{{url}}", "")
                .replace("{{media_type}}", media_type)
                .replace("{{data}}", data)
        }
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(AssistantOutputImagesToMarkdownTransform),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_default_markdown_for_output_images() {
        let cfg = Config { template: None };
        let mut item = Item::Message {
            role: Role::Assistant,
            parts: vec![
                Part::Text {
                    content: "hello".to_string(),
                    extra_body: HashMap::new(),
                },
                Part::Image {
                    source: ImageSource::Url {
                        url: "https://example.com/a.png".to_string(),
                        detail: None,
                    },
                    extra_body: HashMap::new(),
                },
            ],
            extra_body: HashMap::new(),
        };
        append_images_as_markdown(&mut item, &cfg);
        let Item::Message { parts, .. } = item else {
            panic!("expected message");
        };
        let Part::Text { content, .. } = &parts[0] else {
            panic!("expected text");
        };
        assert_eq!(content, "hello![image](https://example.com/a.png)");
    }

    #[test]
    fn supports_custom_template() {
        let cfg = Config {
            template: Some("<img src=\"{{src}}\">".to_string()),
        };
        let rendered = format_image_markdown(
            &ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            },
            &cfg,
        );
        assert_eq!(rendered, "<img src=\"data:image/png;base64,QUJD\">");
    }
}

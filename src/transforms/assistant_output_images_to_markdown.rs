use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{ImageSource, Item, Part, Role, UrpStreamEvent};
use async_trait::async_trait;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
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
                    "description": "Template appended for each image. Supports raw placeholders {{src}}, {{url}}, {{media_type}}, {{data}} and URL-safe placeholders {{src_urlencoded}}, {{url_urlencoded}}, {{media_type_urlencoded}}, {{data_urlencoded}}."
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
                append_images_as_markdown_nodes(&mut resp.output, &cfg);
            }
            UrpData::Stream(event) => match event {
                UrpStreamEvent::ResponseDone { output, .. } => {
                    append_images_as_markdown_nodes(output, &cfg);
                }
                _ => {}
            },
            UrpData::Request(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
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

fn append_images_as_markdown_nodes(output: &mut Vec<crate::urp::Node>, config: &Config) {
    let mut pending_appended = String::new();
    let mut last_text_index: Option<usize> = None;

    for (index, node) in output.iter_mut().enumerate() {
        match node {
            crate::urp::Node::Image {
                role: crate::urp::OrdinaryRole::Assistant,
                source,
                ..
            } => {
                pending_appended.push_str(&format_image_markdown(source, config));
            }
            crate::urp::Node::Text {
                role: crate::urp::OrdinaryRole::Assistant,
                content,
                ..
            } => {
                if !pending_appended.is_empty() {
                    content.push_str(&pending_appended);
                    pending_appended.clear();
                }
                last_text_index = Some(index);
            }
            _ => {}
        }
    }

    if !pending_appended.is_empty() {
        if let Some(index) = last_text_index {
            if let Some(crate::urp::Node::Text { content, .. }) = output.get_mut(index) {
                content.push_str(&pending_appended);
            }
        } else {
            output.push(crate::urp::Node::Text {
                id: None,
                role: crate::urp::OrdinaryRole::Assistant,
                content: pending_appended,
                phase: None,
                extra_body: HashMap::new(),
            });
        }
    }
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
        ImageSource::Url { url, .. } => apply_template(template, url, url, "", ""),
        ImageSource::Base64 { media_type, data } => {
            let src = format!("data:{media_type};base64,{data}");
            apply_template(template, &src, "", media_type, data)
        }
    }
}

fn apply_template(template: &str, src: &str, url: &str, media_type: &str, data: &str) -> String {
    [
        ("{{src_urlencoded}}", percent_encode(src)),
        ("{{url_urlencoded}}", percent_encode(url)),
        ("{{media_type_urlencoded}}", percent_encode(media_type)),
        ("{{data_urlencoded}}", percent_encode(data)),
        ("{{src}}", src.to_string()),
        ("{{url}}", url.to_string()),
        ("{{media_type}}", media_type.to_string()),
        ("{{data}}", data.to_string()),
    ]
    .into_iter()
    .fold(template.to_string(), |rendered, (placeholder, value)| {
        rendered.replace(placeholder, &value)
    })
}

fn percent_encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
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
            id: None,
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
    fn renders_default_data_url_for_base64_output_images() {
        let cfg = Config { template: None };
        let rendered = format_image_markdown(
            &ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            },
            &cfg,
        );
        assert_eq!(rendered, "![image](data:image/png;base64,QUJD)");
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

    #[test]
    fn resolves_url_and_base64_placeholders_by_source_variant() {
        let cfg = Config {
            template: Some(
                "src={{src}} url={{url}} media={{media_type}} data={{data}}".to_string(),
            ),
        };
        let url_rendered = format_image_markdown(
            &ImageSource::Url {
                url: "https://example.com/a.png".to_string(),
                detail: None,
            },
            &cfg,
        );
        assert_eq!(
            url_rendered,
            "src=https://example.com/a.png url=https://example.com/a.png media= data="
        );

        let base64_rendered = format_image_markdown(
            &ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            },
            &cfg,
        );
        assert_eq!(
            base64_rendered,
            "src=data:image/png;base64,QUJD url= media=image/png data=QUJD"
        );
    }

    #[test]
    fn supports_urlencoded_placeholders_for_base64_url_templates() {
        let cfg = Config {
            template: Some(
                "https://cdn.example/render/{{data_urlencoded}}?src={{src_urlencoded}}".to_string(),
            ),
        };
        let rendered = format_image_markdown(
            &ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "ab/c+=,".to_string(),
            },
            &cfg,
        );
        assert_eq!(
            rendered,
            "https://cdn.example/render/ab%2Fc%2B%3D%2C?src=data%3Aimage%2Fpng%3Bbase64%2Cab%2Fc%2B%3D%2C"
        );
    }
}

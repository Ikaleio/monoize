use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{ImageSource, UrpStreamEvent};
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

pub struct ImageOutputToMarkdownTransform;

#[async_trait]
impl Transform for ImageOutputToMarkdownTransform {
    fn type_id(&self) -> &'static str {
        "image_output_to_markdown"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Image: image nodes to Markdown"),
            ("zh", "图像：图像节点转 Markdown"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Renders assistant image nodes as Markdown image links appended to assistant text output. Inverse of image_markdown_to_output.",
            ),
            (
                "zh",
                "将 assistant 图像节点渲染为 Markdown 图片链接并追加到 assistant 文本输出。与 image_markdown_to_output 互逆。",
            ),
        ]
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
                    "format": "multiline",
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
        ImageSource::FileId { .. } => String::new(),
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
        ImageSource::FileId { .. } => String::new(),
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
    factory: || Box::new(ImageOutputToMarkdownTransform),
});

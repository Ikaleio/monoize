use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvgOptions {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<SvgReasoning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvgReasoning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Image request controls shared by the Images API and image-generation tools.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageGenerationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_duration_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svg: Option<SvgOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_compression: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_images: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_fidelity: Option<String>,
}

impl ImageGenerationOptions {
    pub const KEYS: [&'static str; 15] = [
        "n",
        "complexity",
        "frame_count",
        "frame_duration_ms",
        "svg",
        "size",
        "quality",
        "style",
        "response_format",
        "background",
        "output_format",
        "output_compression",
        "moderation",
        "partial_images",
        "input_fidelity",
    ];

    /// Remove recognized controls from extras and validate their common types and ranges.
    pub fn take_from_extra(extra: &mut HashMap<String, Value>) -> Result<Self, String> {
        let fields: Map<String, Value> = Self::KEYS
            .into_iter()
            .filter_map(|key| extra.remove(key).map(|value| (key.to_owned(), value)))
            .collect();
        let options: Self = serde_json::from_value(Value::Object(fields))
            .map_err(|error| format!("invalid image option: {error}"))?;
        options.validate()?;
        Ok(options)
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(level) = self.complexity.as_deref()
            && !matches!(level, "low" | "medium" | "high")
        {
            return Err("complexity must be low, medium, or high".into());
        }
        if self.frame_count.is_some_and(|v| !(2..=16).contains(&v)) {
            return Err("frame_count must be between 2 and 16".into());
        }
        if self
            .frame_duration_ms
            .is_some_and(|v| !(20..=2000).contains(&v))
        {
            return Err("frame_duration_ms must be between 20 and 2000".into());
        }
        if let Some(svg) = &self.svg {
            if svg.model.trim().is_empty() {
                return Err("svg.model must be nonempty".into());
            }
            if let Some(effort) = svg.reasoning.as_ref().and_then(|r| r.effort.as_deref())
                && !matches!(
                    effort,
                    "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                )
            {
                return Err("invalid svg.reasoning.effort".into());
            }
        }
        if self.n == Some(0) {
            return Err("n must be a positive integer".into());
        }
        if self.output_compression.is_some_and(|value| value > 100) {
            return Err("output_compression must be an integer between 0 and 100".into());
        }
        if self.partial_images.is_some_and(|value| value > 3) {
            return Err("partial_images must be an integer between 0 and 3".into());
        }
        Ok(())
    }

    pub fn validate_image_request(
        &self,
        edit: bool,
        stream: bool,
        prompt_rewriter: bool,
    ) -> Result<(), String> {
        self.validate()?;
        let format = self.output_format.as_deref();
        if self.n.is_some_and(|n| n != 1) {
            return Err("n must equal 1".into());
        }
        if (self.frame_count.is_some() || self.frame_duration_ms.is_some()) && format != Some("gif")
        {
            return Err("frame_count and frame_duration_ms require output_format=gif".into());
        }
        if self.svg.is_some() && format != Some("svg") {
            return Err("svg settings require output_format=svg".into());
        }
        if matches!(format, Some("svg" | "gif"))
            && (edit
                || stream
                || self.partial_images.is_some_and(|v| v > 0)
                || self.output_compression.is_some()
                || prompt_rewriter)
        {
            return Err("SVG and GIF require non-streaming generation without partial images, output_compression or prompt_rewriter".into());
        }
        Ok(())
    }

    pub fn to_object(&self) -> Map<String, Value> {
        match serde_json::to_value(self).expect("image options are serializable") {
            Value::Object(fields) => fields,
            _ => unreachable!("image options serialize as an object"),
        }
    }
}

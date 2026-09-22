use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Image request controls shared by the Images API and image-generation tools.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageGenerationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
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
    pub const KEYS: [&'static str; 11] = [
        "n",
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

    pub fn to_object(&self) -> Map<String, Value> {
        match serde_json::to_value(self).expect("image options contain only JSON primitives") {
            Value::Object(fields) => fields,
            _ => unreachable!("image options serialize as an object"),
        }
    }
}

use super::{InputDetails, ModalityBreakdown, OutputDetails, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageIteration {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub usage: Usage,
}

pub fn decode_messages_iterations(value: Option<&Value>) -> Option<Vec<UsageIteration>> {
    let values = value?.as_array()?;
    values
        .iter()
        .map(|value| {
            let mut usage = super::decode::anthropic::decode_usage(value)?;
            usage.iterations = None;
            usage.extra_body.remove("type");
            Some(UsageIteration {
                kind: value.get("type")?.as_str()?.to_owned(),
                usage,
            })
        })
        .collect()
}

fn add_modality(target: &mut Option<ModalityBreakdown>, source: &Option<ModalityBreakdown>) {
    if let Some(source) = source {
        let target = target.get_or_insert_with(Default::default);
        macro_rules! add { ($($field:ident),+) => { $(if let Some(value) = source.$field { target.$field = Some(target.$field.unwrap_or(0).saturating_add(value)); })+ }; }
        add!(
            text_tokens,
            image_tokens,
            audio_tokens,
            video_tokens,
            document_tokens
        );
    }
}

impl Usage {
    /// Returns inclusive operation counters without counting primary-generation counters twice.
    pub fn accounting(&self) -> Cow<'_, Usage> {
        let Some(iterations) = self.iterations.as_ref().filter(|v| !v.is_empty()) else {
            return Cow::Borrowed(self);
        };
        let mut total = Usage {
            extra_body: self.extra_body.clone(),
            ..Usage::default()
        };
        macro_rules! add { ($target:ident,$source:ident;$($field:ident),+) => { $($target.$field = $target.$field.saturating_add($source.$field);)+ }; }
        for iteration in iterations {
            let source = &iteration.usage;
            add!(total,source;input_tokens,output_tokens);
            if let Some(source) = &source.input_details {
                let target = total
                    .input_details
                    .get_or_insert_with(InputDetails::default);
                add!(target,source;standard_tokens,cache_read_tokens,cache_creation_tokens,cache_creation_5m_tokens,cache_creation_1h_tokens,tool_prompt_tokens);
                add_modality(&mut target.modality_breakdown, &source.modality_breakdown);
                add_modality(
                    &mut target.cache_read_modality_breakdown,
                    &source.cache_read_modality_breakdown,
                );
                add_modality(
                    &mut target.tool_prompt_modality_breakdown,
                    &source.tool_prompt_modality_breakdown,
                );
            }
            if let Some(source) = &source.output_details {
                let target = total
                    .output_details
                    .get_or_insert_with(OutputDetails::default);
                add!(target,source;standard_tokens,reasoning_tokens,accepted_prediction_tokens,rejected_prediction_tokens);
                add_modality(&mut target.modality_breakdown, &source.modality_breakdown);
            }
        }
        Cow::Owned(total)
    }
}

pub fn modality(extra: &std::collections::HashMap<String, Value>) -> Option<ModalityBreakdown> {
    let get = |key| extra.get(key).and_then(Value::as_u64);
    let result = ModalityBreakdown {
        text_tokens: get("text_tokens"),
        image_tokens: get("image_tokens"),
        audio_tokens: get("audio_tokens"),
        video_tokens: get("video_tokens"),
        document_tokens: get("document_tokens"),
    };
    [
        result.text_tokens,
        result.image_tokens,
        result.audio_tokens,
        result.video_tokens,
        result.document_tokens,
    ]
    .iter()
    .any(Option::is_some)
    .then_some(result)
}

pub fn strip_modality(extra: &mut std::collections::HashMap<String, Value>) {
    for key in [
        "text_tokens",
        "image_tokens",
        "audio_tokens",
        "video_tokens",
        "document_tokens",
    ] {
        extra.remove(key);
    }
}

pub fn write_modality(value: &mut Value, modality: &Option<ModalityBreakdown>) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in [
        "text_tokens",
        "image_tokens",
        "audio_tokens",
        "video_tokens",
        "document_tokens",
    ] {
        object.remove(key);
    }
    if let Some(modality) = modality {
        if let Ok(Value::Object(fields)) = serde_json::to_value(modality) {
            object.extend(fields);
        }
    }
}

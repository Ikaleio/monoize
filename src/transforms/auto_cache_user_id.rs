use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState,
    UrpData,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct AutoCacheUserIdTransform;

/// When cache fields exist in the request but no user_id is set,
/// auto-fill metadata.user_id (Anthropic) and req.user (OpenAI)
/// with the Monoize username injected via __monoize_username.
impl Transform for AutoCacheUserIdTransform {
    fn type_id(&self) -> &'static str {
        "auto_cache_user_id"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
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

    fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        let username = match req
            .extra_body
            .get("__monoize_username")
            .and_then(|v| v.as_str())
        {
            Some(u) => u.to_string(),
            None => return Ok(()),
        };

        if !has_any_cache_control(req) {
            return Ok(());
        }

        // Anthropic: set metadata.user_id if not already present
        let metadata = req
            .extra_body
            .entry("metadata".to_string())
            .or_insert_with(|| json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.entry("user_id".to_string())
                .or_insert_with(|| json!(username));
        }

        // OpenAI: set req.user if not already present
        if req.user.is_none() {
            req.user = Some(username);
        }

        Ok(())
    }
}

fn has_any_cache_control(req: &crate::urp::UrpRequest) -> bool {
    req.messages.iter().any(|msg| {
        msg.parts
            .iter()
            .any(|part| part_extra_body(part).map_or(false, |eb| eb.contains_key("cache_control")))
    })
}

fn part_extra_body(part: &crate::urp::Part) -> Option<&std::collections::HashMap<String, Value>> {
    match part {
        crate::urp::Part::Text { extra_body, .. }
        | crate::urp::Part::Image { extra_body, .. }
        | crate::urp::Part::Audio { extra_body, .. }
        | crate::urp::Part::File { extra_body, .. }
        | crate::urp::Part::Reasoning { extra_body, .. }
        | crate::urp::Part::ReasoningEncrypted { extra_body, .. }
        | crate::urp::Part::ToolCall { extra_body, .. }
        | crate::urp::Part::ToolResult { extra_body, .. }
        | crate::urp::Part::Refusal { extra_body, .. } => Some(extra_body),
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(AutoCacheUserIdTransform),
});

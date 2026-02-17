use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState,
    UrpData, model_glob_match,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct SuffixRule {
    pattern: String,
    suffix: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    rules: Vec<SuffixRule>,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ReasoningEffortToModelSuffixTransform;

impl Transform for ReasoningEffortToModelSuffixTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_effort_to_model_suffix"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "rules": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "pattern": { "type": "string", "minLength": 1 },
                            "suffix": { "type": "string", "minLength": 1 }
                        },
                        "required": ["pattern", "suffix"],
                        "additionalProperties": false
                    },
                    "minItems": 1
                }
            },
            "required": ["rules"],
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        if cfg.rules.is_empty() {
            return Err(TransformError::InvalidConfig(
                "rules must not be empty".to_string(),
            ));
        }
        Ok(Box::new(cfg))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(NoState)
    }

    fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        let UrpData::Request(req) = data else {
            return Ok(());
        };
        let effort = match req.reasoning.as_ref().and_then(|r| r.effort.as_deref()) {
            Some(e @ ("low" | "medium" | "high")) => e.to_string(),
            _ => return Ok(()),
        };
        for rule in &cfg.rules {
            if model_glob_match(&rule.pattern, &req.model) {
                let suffix = rule.suffix.replace("{effort}", &effort);
                req.model.push_str(&suffix);
                return Ok(());
            }
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningEffortToModelSuffixTransform),
});

use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState,
    UrpData, set_extra_path,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {
    low: u32,
    med: u32,
    high: u32,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ReasoningEffortToBudgetTransform;

impl Transform for ReasoningEffortToBudgetTransform {
    fn type_id(&self) -> &'static str {
        "reasoning_effort_to_budget"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "low": { "type": "integer", "minimum": 0 },
                "med": { "type": "integer", "minimum": 0 },
                "high": { "type": "integer", "minimum": 0 }
            },
            "required": ["low", "med", "high"],
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
        let Some(reasoning) = req.reasoning.as_ref() else {
            return Ok(());
        };
        let Some(effort) = reasoning.effort.as_deref() else {
            return Ok(());
        };
        let budget = match effort {
            "low" => cfg.low,
            "medium" => cfg.med,
            "high" => cfg.high,
            _ => return Ok(()),
        };
        set_extra_path(
            &mut req.extra_body,
            "thinking.budget_tokens",
            Value::Number(serde_json::Number::from(budget)),
        );
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(ReasoningEffortToBudgetTransform),
});

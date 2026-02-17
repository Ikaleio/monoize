use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError, TransformState,
    UrpData, set_extra_path,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

#[derive(Debug, Deserialize)]
struct Config {
    path: String,
    value: Value,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct SetFieldTransform;

impl Transform for SetFieldTransform {
    fn type_id(&self) -> &'static str {
        "set_field"
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request, Phase::Response]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1 },
                "value": {}
            },
            "required": ["path", "value"],
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
        match data {
            UrpData::Request(req) => {
                set_extra_path(&mut req.extra_body, &cfg.path, cfg.value.clone())
            }
            UrpData::Response(resp) => {
                set_extra_path(&mut resp.extra_body, &cfg.path, cfg.value.clone())
            }
            UrpData::Stream(event) => match event {
                crate::urp::UrpStreamEvent::ResponseStart { extra_body, .. }
                | crate::urp::UrpStreamEvent::PartStart { extra_body, .. }
                | crate::urp::UrpStreamEvent::Delta { extra_body, .. }
                | crate::urp::UrpStreamEvent::PartDone { extra_body, .. }
                | crate::urp::UrpStreamEvent::ResponseDone { extra_body, .. }
                | crate::urp::UrpStreamEvent::Error { extra_body, .. } => {
                    set_extra_path(extra_body, &cfg.path, cfg.value.clone());
                }
            },
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(SetFieldTransform),
});

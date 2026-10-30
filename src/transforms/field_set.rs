use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformState, UrpData, set_extra_path,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Config {
    path: String,
    value: Value,
    #[serde(default)]
    when_equals: Option<Value>,
}

fn extra_path_value<'a>(extra: &'a HashMap<String, Value>, path: &str) -> Option<&'a Value> {
    let mut keys = path.split('.').filter(|key| !key.is_empty());
    let first = keys.next()?;
    let mut current = extra.get(first)?;
    for key in keys {
        current = current.as_object()?.get(key)?;
    }
    Some(current)
}

fn field_set(extra: &mut HashMap<String, Value>, path: &str, config: &Config) {
    if config
        .when_equals
        .as_ref()
        .is_none_or(|expected| extra_path_value(extra, path) == Some(expected))
    {
        set_extra_path(extra, path, config.value.clone());
    }
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct FieldSetTransform;

#[async_trait]
impl Transform for FieldSetTransform {
    fn type_id(&self) -> &'static str {
        "field_set"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[("en", "Field: set"), ("zh", "字段：设置")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Writes a JSON value at the configured extra-body path, optionally only when the current value equals when_equals.",
            ),
            (
                "zh",
                "在配置路径写入 JSON 值；配置 when_equals 时仅当当前值与其相等才写入。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request, Phase::Response]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1 },
                "value": {
                    "description": "JSON value written at path. Example: \"normal\".",
                    "x-localized-description": {
                        "en": "JSON value written at path. Example: \"normal\".",
                        "zh": "写入 path 的 JSON 值。示例：\"normal\"。",
                        "zh-TW": "寫入 path 的 JSON 值。範例：\"normal\"。",
                        "ja": "path に書き込む JSON 値。例：\"normal\"。"
                    }
                },
                "when_equals": {
                    "description": "Optional. When set, write only if the current value equals this JSON value. Leave empty to always write.",
                    "x-localized-description": {
                        "en": "Optional. When set, write only if the current value equals this JSON value. Leave empty to always write.",
                        "zh": "可选。设置后，仅当当前值等于此 JSON 值时才写入。不设置则始终写入。",
                        "zh-TW": "選填。設定後，僅當目前值等於此 JSON 值時才寫入。不設定則一律寫入。",
                        "ja": "任意。設定すると、現在の値がこの JSON 値と等しい場合のみ書き込みます。未設定なら常に書き込みます。"
                    }
                }
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
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;
        match data {
            UrpData::Request(req) => {
                if let Some(sub_path) = cfg.path.strip_prefix("reasoning.") {
                    if cfg.when_equals.as_ref().is_some_and(|expected| {
                        req.reasoning.as_ref().and_then(|reasoning| {
                            if crate::urp::ReasoningConfig::is_control(sub_path) {
                                reasoning.control(sub_path)
                            } else {
                                extra_path_value(&reasoning.extra_body, sub_path).cloned()
                            }
                        }) != Some(expected.clone())
                    }) {
                        return Ok(());
                    }
                    let reasoning = req.reasoning.get_or_insert_with(Default::default);
                    if !reasoning
                        .set_control(sub_path, Some(cfg.value.clone()))
                        .map_err(TransformError::Apply)?
                    {
                        set_extra_path(&mut reasoning.extra_body, sub_path, cfg.value.clone());
                    }
                } else {
                    field_set(&mut req.extra_body, &cfg.path, cfg);
                }
            }
            UrpData::Response(resp) => field_set(&mut resp.extra_body, &cfg.path, cfg),
            UrpData::Stream(event) => match event {
                crate::urp::UrpStreamEvent::ResponseStart { extra_body, .. }
                | crate::urp::UrpStreamEvent::ResponseDone { extra_body, .. }
                | crate::urp::UrpStreamEvent::NodeStart { extra_body, .. }
                | crate::urp::UrpStreamEvent::NodeDelta { extra_body, .. }
                | crate::urp::UrpStreamEvent::NodeDone { extra_body, .. }
                | crate::urp::UrpStreamEvent::ProviderControl { extra_body, .. }
                | crate::urp::UrpStreamEvent::Error { extra_body, .. } => {
                    field_set(extra_body, &cfg.path, cfg);
                }
            },
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(FieldSetTransform),
});

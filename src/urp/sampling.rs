use super::ProviderProtocol;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

const FIELDS: &[&str] = &["top_k", "seed", "presence_penalty", "frequency_penalty"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SamplingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
}

/// Decodes supported source controls, or returns None when all controls are absent.
pub fn request_config(
    body: &Map<String, Value>,
    protocol: ProviderProtocol,
) -> Option<SamplingConfig> {
    if !matches!(
        protocol,
        ProviderProtocol::ChatCompletion | ProviderProtocol::Messages
    ) {
        return None;
    }
    let mut config = SamplingConfig {
        top_k: body
            .get("top_k")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        ..SamplingConfig::default()
    };
    if protocol == ProviderProtocol::ChatCompletion {
        config.seed = body.get("seed").and_then(Value::as_i64);
        config.presence_penalty = body.get("presence_penalty").and_then(Value::as_f64);
        config.frequency_penalty = body.get("frequency_penalty").and_then(Value::as_f64);
    }
    (config != SamplingConfig::default()).then_some(config)
}

/// Removes source controls whose values belong to SamplingConfig.
pub fn strip_request_extras(extra: &mut HashMap<String, Value>, protocol: ProviderProtocol) {
    let fields = match protocol {
        ProviderProtocol::ChatCompletion => FIELDS,
        ProviderProtocol::Messages => &FIELDS[..1],
        _ => return,
    };
    for field in fields {
        extra.remove(*field);
    }
}

/// Replaces native controls with typed values and omits unsupported target controls.
pub fn encode_request(
    body: &mut Value,
    config: &Option<SamplingConfig>,
    protocol: ProviderProtocol,
) {
    let Some(body) = body.as_object_mut() else {
        return;
    };
    for field in FIELDS {
        body.remove(*field);
    }
    let Some(config) = config else {
        return;
    };
    if matches!(
        protocol,
        ProviderProtocol::ChatCompletion | ProviderProtocol::Messages
    ) {
        if let Some(value) = config.top_k {
            body.insert("top_k".into(), Value::from(value));
        }
    }
    if protocol == ProviderProtocol::ChatCompletion {
        if let Some(value) = config.seed {
            body.insert("seed".into(), Value::from(value));
        }
        if let Some(value) = config.presence_penalty {
            body.insert("presence_penalty".into(), Value::from(value));
        }
        if let Some(value) = config.frequency_penalty {
            body.insert("frequency_penalty".into(), Value::from(value));
        }
    }
}

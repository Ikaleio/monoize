use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogprobConfig {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenScore {
    pub token: String,
    #[serde(default)]
    pub bytes: Option<Vec<u8>>,
    pub logprob: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenLogprob {
    #[serde(flatten)]
    pub score: TokenScore,
    #[serde(default)]
    pub top_logprobs: Vec<TokenScore>,
}

pub fn decode(value: Option<&Value>) -> Option<Vec<TokenLogprob>> {
    value
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn valid<'a>(scores: &'a Option<Vec<TokenLogprob>>, text: &str) -> Option<&'a [TokenLogprob]> {
    let scores = scores.as_ref()?;
    let bytes: Vec<u8> = scores
        .iter()
        .flat_map(|v| {
            v.score
                .bytes
                .as_deref()
                .unwrap_or(v.score.token.as_bytes())
                .iter()
                .copied()
        })
        .collect();
    (bytes == text.as_bytes()).then_some(scores.as_slice())
}

pub fn append(target: &mut Option<Vec<TokenLogprob>>, delta: &Option<Vec<TokenLogprob>>) {
    if let Some(delta) = delta {
        target
            .get_or_insert_with(Vec::new)
            .extend(delta.iter().cloned());
    }
}

pub fn request_config(
    body: &serde_json::Map<String, Value>,
    protocol: super::ProviderProtocol,
) -> Option<LogprobConfig> {
    let top_k = body
        .get("top_logprobs")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    let enabled = if protocol == super::ProviderProtocol::ChatCompletion {
        body.get("logprobs").and_then(Value::as_bool)
    } else {
        body.get("include")
            .and_then(Value::as_array)
            .filter(|values| {
                values
                    .iter()
                    .any(|v| v.as_str() == Some("message.output_text.logprobs"))
            })
            .map(|_| true)
    };
    enabled
        .or(top_k.map(|_| true))
        .map(|enabled| LogprobConfig { enabled, top_k })
}

pub fn strip_request_extras(extra: &mut std::collections::HashMap<String, Value>) {
    extra.remove("logprobs");
    extra.remove("top_logprobs");
    if let Some(Value::Array(values)) = extra.get_mut("include") {
        values.retain(|v| v.as_str() != Some("message.output_text.logprobs"));
        if values.is_empty() {
            extra.remove("include");
        }
    }
}

pub fn encode_request(
    body: &mut Value,
    config: &Option<LogprobConfig>,
    protocol: super::ProviderProtocol,
) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    obj.remove("logprobs");
    obj.remove("top_logprobs");
    if let Some(Value::Array(values)) = obj.get_mut("include") {
        values.retain(|v| v.as_str() != Some("message.output_text.logprobs"));
    }
    if let Some(config) = config {
        if protocol == super::ProviderProtocol::ChatCompletion {
            obj.insert("logprobs".into(), Value::Bool(config.enabled));
        } else if config.enabled {
            obj.entry("include")
                .or_insert_with(|| Value::Array(vec![]))
                .as_array_mut()
                .map(|v| v.push(Value::String("message.output_text.logprobs".into())));
        }
        if let Some(top_k) = config.top_k {
            obj.insert("top_logprobs".into(), Value::from(top_k));
        }
    }
}

impl super::Node {
    pub fn token_scores(&self) -> Option<&[TokenLogprob]> {
        match self {
            Self::Text {
                content, logprobs, ..
            }
            | Self::Refusal {
                content, logprobs, ..
            } => valid(logprobs, content),
            _ => None,
        }
    }
}

/// Groups complete tokens until their bytes form valid UTF-8; a token is never duplicated across frames.
pub fn fragments(scores: &[TokenLogprob]) -> Vec<(String, Vec<TokenLogprob>)> {
    let mut bytes = Vec::new();
    let mut pending = Vec::new();
    let mut result = Vec::new();
    for score in scores {
        bytes.extend_from_slice(
            score
                .score
                .bytes
                .as_deref()
                .unwrap_or(score.score.token.as_bytes()),
        );
        pending.push(score.clone());
        if let Ok(text) = std::str::from_utf8(&bytes) {
            result.push((text.to_owned(), std::mem::take(&mut pending)));
            bytes.clear();
        }
    }
    result
}

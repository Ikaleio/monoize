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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<u64>,
    #[serde(default)]
    pub bytes: Option<Vec<u8>>,
    pub logprob: f64,
}

/// Attaches Gemini token scores only when complete tokens match each owning text node.
pub fn attach_gemini(candidate: &serde_json::Map<String, Value>, nodes: &mut [super::Node]) {
    let Some(result) = candidate.get("logprobsResult") else {
        return;
    };
    let Some(chosen) = result.get("chosenCandidates").and_then(Value::as_array) else {
        return;
    };
    let alternatives = result.get("topCandidates").and_then(Value::as_array);
    let mut scores = Vec::with_capacity(chosen.len());
    for (index, value) in chosen.iter().enumerate() {
        let Some(score) = gemini_score(value) else {
            return;
        };
        let top_logprobs = alternatives
            .and_then(|values| values.get(index))
            .and_then(|entry| entry.get("candidates"))
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(gemini_score).collect())
            .unwrap_or_default();
        scores.push(TokenLogprob {
            score,
            top_logprobs,
        });
    }
    let text: String = nodes
        .iter()
        .filter_map(|node| match node {
            super::Node::Text { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    if valid(&Some(scores.clone()), &text).is_none() {
        return;
    }
    let mut cursor = 0;
    let mut placements = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let super::Node::Text { content, .. } = node else {
            continue;
        };
        let start = cursor;
        let mut bytes = 0;
        while cursor < scores.len() && bytes < content.len() {
            bytes += scores[cursor].score.token.len();
            cursor += 1;
        }
        if bytes != content.len() {
            return;
        }
        placements.push((index, start..cursor));
    }
    for (index, range) in placements {
        if let super::Node::Text { logprobs, .. } = &mut nodes[index] {
            *logprobs = Some(scores[range].to_vec());
        }
    }
}

fn gemini_score(value: &Value) -> Option<TokenScore> {
    Some(TokenScore {
        token: value.get("token")?.as_str()?.to_owned(),
        token_id: value.get("tokenId").and_then(Value::as_u64),
        bytes: None,
        logprob: value.get("logProbability")?.as_f64()?,
    })
}

fn encode_gemini_score(score: &TokenScore) -> Option<Value> {
    let token = match &score.bytes {
        Some(bytes) => std::str::from_utf8(bytes).ok()?,
        None => &score.token,
    };
    let mut value = serde_json::json!({"token":token,"logProbability":score.logprob});
    if let Some(id) = score.token_id {
        value["tokenId"] = Value::from(id);
    }
    Some(value)
}

fn gemini_scores(nodes: &[super::Node]) -> Option<Vec<&TokenLogprob>> {
    let mut result = Vec::new();
    for node in nodes {
        if let super::Node::Text {
            content, logprobs, ..
        } = node
        {
            if !content.is_empty() {
                result.extend(valid(logprobs, content)?);
            }
        }
    }
    (!result.is_empty()).then_some(result)
}

/// Encodes candidate scores from current typed text, omitting invalidated token scores.
pub fn encode_gemini(nodes: &[super::Node]) -> Option<Value> {
    let scores = gemini_scores(nodes)?;
    let chosen = scores
        .iter()
        .map(|v| encode_gemini_score(&v.score))
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::json!({
        "chosenCandidates":chosen,
        "topCandidates":scores.iter().map(|v| serde_json::json!({"candidates":v.top_logprobs.iter().filter_map(encode_gemini_score).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "logProbabilitySum":scores.iter().map(|v|v.score.logprob).sum::<f64>()
    }))
}

pub fn gemini_summary(nodes: &[super::Node]) -> Option<f64> {
    let scores = gemini_scores(nodes)?;
    Some(scores.iter().map(|v| v.score.logprob).sum::<f64>() / scores.len() as f64)
}

/// Projects shared scores onto the OpenAI token-score fields.
pub fn encode_openai(scores: &[TokenLogprob]) -> Value {
    let score = |score: &TokenScore| {
        serde_json::json!({
            "token":score.token,"bytes":score.bytes,"logprob":score.logprob
        })
    };
    Value::Array(
        scores
            .iter()
            .map(|entry| {
                let mut value = score(&entry.score);
                value["top_logprobs"] =
                    Value::Array(entry.top_logprobs.iter().map(&score).collect());
                value
            })
            .collect(),
    )
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

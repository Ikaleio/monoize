use super::FinishReason;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Completed,
    Incomplete,
    Failed,
    Cancelled,
    Queued,
    InProgress,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseOutcome {
    pub status: ResponseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_reason: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub incomplete_extra: HashMap<String, Value>,
}

impl ResponseOutcome {
    pub fn from_responses(body: &Map<String, Value>) -> Option<Self> {
        let status = serde_json::from_value(body.get("status")?.clone()).ok()?;
        let error = body
            .get("error")
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let details = body.get("incomplete_details").and_then(Value::as_object);
        Some(Self {
            status,
            error,
            incomplete_reason: details
                .and_then(|v| v.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            incomplete_extra: details
                .into_iter()
                .flatten()
                .filter(|(k, _)| k.as_str() != "reason" && !k.starts_with("_monoize_"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    pub fn from_finish(reason: Option<FinishReason>) -> Self {
        let (status, detail) = match reason {
            Some(FinishReason::Length) => (ResponseStatus::Incomplete, Some("max_output_tokens")),
            Some(FinishReason::ContextLimit) => {
                (ResponseStatus::Incomplete, Some("context_length_exceeded"))
            }
            Some(FinishReason::Paused) => (ResponseStatus::Incomplete, Some("pause_turn")),
            Some(FinishReason::Compaction) => (ResponseStatus::Incomplete, Some("compaction")),
            Some(FinishReason::ContentFilter) => {
                (ResponseStatus::Incomplete, Some("content_filter"))
            }
            Some(FinishReason::Other) => (ResponseStatus::Failed, None),
            _ => (ResponseStatus::Completed, None),
        };
        Self {
            status,
            error: None,
            incomplete_reason: detail.map(str::to_owned),
            incomplete_extra: HashMap::new(),
        }
    }

    pub fn failure_body(&self, messages: bool) -> Option<Value> {
        if !matches!(
            self.status,
            ResponseStatus::Failed | ResponseStatus::Cancelled
        ) {
            return None;
        }
        let mut error = self
            .error
            .as_ref()
            .map(|e| json!(e))
            .unwrap_or_else(|| json!({}));
        if error.get("message").and_then(Value::as_str).is_none() {
            error["message"] = json!(if self.status == ResponseStatus::Cancelled {
                "Upstream response was cancelled"
            } else {
                "Upstream response failed"
            });
        }
        if messages {
            error["type"] = json!("api_error");
        }
        let mut body = json!({"error":error});
        if messages {
            body["type"] = json!("error");
        }
        Some(body)
    }

    pub fn write_responses(&self, body: &mut Value) {
        body["status"] = json!(self.status);
        body["error"] = json!(self.error);
        body["incomplete_details"] = if self.status == ResponseStatus::Incomplete {
            let mut details: Map<String, Value> =
                self.incomplete_extra.clone().into_iter().collect();
            if let Some(reason) = &self.incomplete_reason {
                details.insert("reason".into(), json!(reason));
            }
            Value::Object(details)
        } else {
            Value::Null
        };
    }
}

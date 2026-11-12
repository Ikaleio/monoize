use super::{
    CacheBreakpoint, ImageGenerationOptions, LogprobConfig, Node, ProviderProtocol, RequestContext,
    SamplingConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopControl {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrpRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_generation: Option<ImageGenerationOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<LogprobConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingConfig>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_format: Option<InstructionsFormat>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "super::context::discard_wire_context"
    )]
    pub context: RequestContext,
    pub input: Vec<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionsFormat {
    Text,
    Items,
    Null,
}

impl ReasoningConfig {
    pub fn is_control(field: &str) -> bool {
        matches!(
            field,
            "effort" | "summary" | "mode" | "budget_tokens" | "display"
        )
    }

    pub fn disabled(&self) -> bool {
        self.effort.as_deref() == Some("none") || self.mode.as_deref() == Some("disabled")
    }

    pub fn control(&self, field: &str) -> Option<Value> {
        match field {
            "effort" => self.effort.clone().map(Value::String),
            "summary" => self.summary.clone().map(Value::String),
            "mode" => self.mode.clone().map(Value::String),
            "display" => self.display.clone().map(Value::String),
            "budget_tokens" => self.budget_tokens.map(Value::from),
            _ => None,
        }
    }

    pub fn set_control(&mut self, field: &str, value: Option<Value>) -> Result<bool, String> {
        if field == "budget_tokens" {
            self.budget_tokens = match value.filter(|v| !v.is_null()) {
                None => None,
                Some(v) => Some(
                    v.as_u64()
                        .ok_or("reasoning.budget_tokens must be an unsigned integer")?,
                ),
            };
            return Ok(true);
        }
        let target = match field {
            "effort" => &mut self.effort,
            "summary" => &mut self.summary,
            "mode" => &mut self.mode,
            "display" => &mut self.display,
            _ => return Ok(false),
        };
        *target = match value.filter(|v| !v.is_null()) {
            None => None,
            Some(Value::String(v)) => Some(v),
            Some(_) => return Err(format!("reasoning.{field} must be a string")),
        };
        Ok(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_protocol: Option<ProviderProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_breakpoint: Option<CacheBreakpoint>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Value>,
    #[serde(default, flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Mode(String),
    Specific(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: JsonSchemaDefinition },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchemaDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

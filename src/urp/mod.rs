use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub mod decode;
pub mod encode;
pub mod greedy;
pub mod stream_decode;
pub mod stream_encode;
pub mod stream_helpers;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrpRequest {
    pub model: String,
    pub inputs: Vec<Item>,
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
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    Message {
        role: Role,
        parts: Vec<Part>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ToolResult {
        call_id: String,
        #[serde(default)]
        is_error: bool,
        content: Vec<ToolResultContent>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        content: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Image {
        source: ImageSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Audio {
        source: AudioSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        source: FileSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Refusal {
        content: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Url {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Base64 {
        media_type: String,
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioSource {
    Url { url: String },
    Base64 { media_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSource {
    Url {
        url: String,
    },
    Base64 {
        filename: Option<String>,
        media_type: String,
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
    Image { source: ImageSource },
    File { source: FileSource },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionDefinition>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(flatten)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrpResponse {
    pub id: String,
    pub model: String,
    pub outputs: Vec<Item>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModalityBreakdown {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputDetails {
    #[serde(default)]
    pub standard_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub tool_prompt_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modality_breakdown: Option<ModalityBreakdown>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputDetails {
    #[serde(default)]
    pub standard_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub accepted_prediction_tokens: u64,
    #[serde(default)]
    pub rejected_prediction_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modality_breakdown: Option<ModalityBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_details: Option<InputDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_details: Option<OutputDetails>,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn cached_tokens(&self) -> Option<u64> {
        self.input_details
            .as_ref()
            .map(|d| d.cache_read_tokens)
            .filter(|&v| v > 0)
    }

    pub fn reasoning_tokens(&self) -> Option<u64> {
        self.output_details
            .as_ref()
            .map(|d| d.reasoning_tokens)
            .filter(|&v| v > 0)
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UrpStreamEvent {
    ResponseStart {
        id: String,
        model: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ItemStart {
        item_index: u32,
        header: ItemHeader,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    PartStart {
        part_index: u32,
        item_index: u32,
        header: PartHeader,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Delta {
        part_index: u32,
        delta: PartDelta,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    PartDone {
        part_index: u32,
        part: Part,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ItemDone {
        item_index: u32,
        item: Item,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ResponseDone {
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        outputs: Vec<Item>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Error {
        code: Option<String>,
        message: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemHeader {
    Message { role: Role },
    ToolResult { call_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartHeader {
    Text,
    Reasoning,
    Refusal,
    ToolCall {
        call_id: String,
        name: String,
    },
    Image {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Audio {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        item_type: String,
        body: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartDelta {
    Text {
        content: String,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    Refusal {
        content: String,
    },
    ToolCallArguments {
        arguments: String,
    },
    Image {
        source: ImageSource,
    },
    Audio {
        source: AudioSource,
    },
    File {
        source: FileSource,
    },
    ProviderItem {
        data: Value,
    },
}

impl Item {
    pub fn new_message(role: Role) -> Self {
        Item::Message {
            role,
            parts: Vec::new(),
            extra_body: HashMap::new(),
        }
    }

    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Item::Message {
            role,
            parts: vec![Part::Text {
                content: content.into(),
                extra_body: HashMap::new(),
            }],
            extra_body: HashMap::new(),
        }
    }
}

pub fn content_text(parts: &[Part]) -> String {
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Text { content, .. } | Part::Refusal { content, .. } => out.push_str(content),
            Part::Reasoning {
                content, summary, ..
            } => {
                if let Some(content) = content {
                    out.push_str(content);
                } else if let Some(summary) = summary {
                    out.push_str(summary);
                }
            }
            _ => {}
        }
    }
    out
}

pub fn output_items(outputs: &[Item]) -> impl Iterator<Item = &Item> {
    outputs.iter()
}

pub fn output_items_mut(outputs: &mut [Item]) -> impl Iterator<Item = &mut Item> {
    outputs.iter_mut()
}

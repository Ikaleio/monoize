use super::{CacheBreakpoint, Citation, TokenLogprob};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    Responses,
    ChatCompletion,
    Messages,
    Gemini,
    OpenaiImage,
    OpenrouterImage,
    Replicate,
}

impl ProviderProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderProtocol::Responses => "responses",
            ProviderProtocol::ChatCompletion => "chat_completion",
            ProviderProtocol::Messages => "messages",
            ProviderProtocol::Gemini => "gemini",
            ProviderProtocol::OpenaiImage => "openai_image",
            ProviderProtocol::OpenrouterImage => "openrouter_image",
            ProviderProtocol::Replicate => "replicate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<TokenLogprob>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        citations: Vec<Citation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_breakpoint: Option<CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Image {
        #[serde(default)]
        metadata: MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        source: ImageSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Audio {
        #[serde(default)]
        metadata: MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        source: AudioSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        #[serde(default)]
        metadata: MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: OrdinaryRole,
        source: FileSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Refusal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<TokenLogprob>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        content: String,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Reasoning {
        #[serde(default)]
        metadata: ReasoningMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
        name: String,
        arguments: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_breakpoint: Option<CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        origin_protocol: ProviderProtocol,
        role: OrdinaryRole,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        tool_type: ToolCallType,
        call_id: String,
        #[serde(default)]
        is_error: bool,
        content: Vec<ToolResultContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_breakpoint: Option<CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    NextDownstreamEnvelopeExtra {
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrdinaryRole {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallType {
    #[default]
    Function,
    Custom,
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
    FileId {
        file_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
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
    Url { url: String },
    FileId { file_id: String },
    Text { text: String },
    Content { content: Vec<Value> },
    Base64 { media_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_breakpoint: Option<CacheBreakpoint>,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    Image {
        #[serde(default)]
        metadata: MediaMetadata,
        source: ImageSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    File {
        #[serde(default)]
        metadata: MediaMetadata,
        source: FileSource,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
    ProviderItem {
        origin_protocol: ProviderProtocol,
        item_type: String,
        body: Value,
        #[serde(flatten)]
        extra_body: HashMap<String, Value>,
    },
}

impl ToolResultContent {
    pub fn extra_body_mut(&mut self) -> &mut HashMap<String, Value> {
        match self {
            Self::Text { extra_body, .. }
            | Self::Image { extra_body, .. }
            | Self::File { extra_body, .. }
            | Self::ProviderItem { extra_body, .. } => extra_body,
        }
    }

    pub fn cache_breakpoint(&self) -> Option<&CacheBreakpoint> {
        match self {
            Self::Text {
                cache_breakpoint, ..
            } => cache_breakpoint.as_ref(),
            Self::Image { metadata, .. } | Self::File { metadata, .. } => {
                metadata.cache_breakpoint.as_ref()
            }
            Self::ProviderItem { .. } => None,
        }
    }

    pub fn cache_breakpoint_mut(&mut self) -> Option<&mut Option<CacheBreakpoint>> {
        match self {
            Self::Text {
                cache_breakpoint, ..
            } => Some(cache_breakpoint),
            Self::Image { metadata, .. } | Self::File { metadata, .. } => {
                Some(&mut metadata.cache_breakpoint)
            }
            Self::ProviderItem { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub image_mask: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub image_markup: bool,
    #[serde(default, skip_serializing_if = "ImageGenerationMetadata::is_empty")]
    pub image_generation: ImageGenerationMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_citations: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<MediaResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_breakpoint: Option<CacheBreakpoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SvgUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageGenerationMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svg_usage: Option<SvgUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ImageGenerationMetadata {
    pub const KEYS: [&'static str; 8] = [
        "svg_usage",
        "safety_identifier",
        "quality",
        "size",
        "background",
        "output_format",
        "model",
        "revised_prompt",
    ];

    pub fn from_object(obj: &serde_json::Map<String, Value>) -> Self {
        let string = |key| obj.get(key).and_then(Value::as_str).map(str::to_owned);
        Self {
            svg_usage: obj
                .get("svg_usage")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
            safety_identifier: string("safety_identifier"),
            revised_prompt: string("revised_prompt"),
            quality: string("quality"),
            size: string("size"),
            background: string("background"),
            output_format: string("output_format"),
            model: string("model"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.svg_usage.is_none()
            && self.safety_identifier.is_none()
            && self.revised_prompt.is_none()
            && self.quality.is_none()
            && self.size.is_none()
            && self.background.is_none()
            && self.output_format.is_none()
            && self.model.is_none()
    }

    pub fn common_response_fields(&self) -> serde_json::Map<String, Value> {
        let mut fields: serde_json::Map<String, Value> = [
            ("safety_identifier", &self.safety_identifier),
            ("quality", &self.quality),
            ("size", &self.size),
            ("background", &self.background),
            ("output_format", &self.output_format),
            ("model", &self.model),
        ]
        .into_iter()
        .filter_map(|(key, value)| {
            value
                .as_ref()
                .map(|value| (key.to_owned(), Value::String(value.clone())))
        })
        .collect();
        if let Some(usage) = &self.svg_usage {
            fields.insert("svg_usage".into(), serde_json::json!(usage));
        }
        fields
    }

    pub fn apply_to(&self, obj: &mut serde_json::Map<String, Value>) {
        for key in Self::KEYS {
            obj.remove(key);
        }
        obj.extend(self.common_response_fields());
        if let Some(prompt) = &self.revised_prompt {
            obj.insert("revised_prompt".to_owned(), Value::String(prompt.clone()));
        }
    }

    pub fn for_source(&self, source: &ImageSource) -> Self {
        let mut generation = self.clone();
        if generation.output_format.is_some()
            && let ImageSource::Base64 { media_type, .. } = source
        {
            let format = match media_type.as_str() {
                "image/png" => Some("png"),
                "image/jpeg" => Some("jpeg"),
                "image/webp" => Some("webp"),
                "image/svg+xml" => Some("svg"),
                "image/gif" => Some("gif"),
                _ => None,
            };
            if let Some(format) = format {
                generation.output_format = Some(format.to_owned());
            }
        }
        generation
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaResource {
    pub protocol: ProviderProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_scope: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReasoningMetadata {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub downstream_only: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chat_content: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summary_as_thinking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_parts: Option<Vec<ReasoningTextPart>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<ReasoningTextPart>>,
    /// Native `summary_index` of a streamed summary delta (STR3i.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_index: Option<u64>,
    /// An identified upstream Responses item with no payload (PR2g.1).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_empty: bool,
    /// Native summary part lifecycle signal on a delta without summary text (STR3i.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_event: Option<ReasoningSummaryEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSummaryEvent {
    PartAdded { part: Value },
    TextDone { text: String },
    PartDone { part: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningTextPart {
    pub byte_length: usize,
    #[serde(flatten)]
    pub extra_body: HashMap<String, Value>,
}

impl Node {
    pub fn text(role: OrdinaryRole, content: impl Into<String>) -> Self {
        Node::Text {
            logprobs: None,
            signature: None,
            citations: Vec::new(),
            id: None,
            role,
            content: content.into(),
            phase: None,
            extra_body: HashMap::new(),
            cache_breakpoint: None,
        }
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::text(OrdinaryRole::Assistant, content)
    }

    pub fn role(&self) -> Option<OrdinaryRole> {
        match self {
            Node::Text { role, .. }
            | Node::Image { role, .. }
            | Node::Audio { role, .. }
            | Node::File { role, .. }
            | Node::ProviderItem { role, .. } => Some(*role),
            Node::Refusal { .. } | Node::Reasoning { .. } | Node::ToolCall { .. } => {
                Some(OrdinaryRole::Assistant)
            }
            Node::ToolResult { .. } | Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    pub fn cache_breakpoint(&self) -> Option<&CacheBreakpoint> {
        match self {
            Node::Text {
                cache_breakpoint, ..
            }
            | Node::ToolCall {
                cache_breakpoint, ..
            }
            | Node::ToolResult {
                cache_breakpoint, ..
            } => cache_breakpoint.as_ref(),
            Node::Image { metadata, .. }
            | Node::Audio { metadata, .. }
            | Node::File { metadata, .. } => metadata.cache_breakpoint.as_ref(),
            Node::Refusal { .. }
            | Node::Reasoning { .. }
            | Node::ProviderItem { .. }
            | Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    /// The marker slot of a carrier, or `None` for a node that cannot hold a marker (CB-2).
    pub fn cache_breakpoint_mut(&mut self) -> Option<&mut Option<CacheBreakpoint>> {
        match self {
            Node::Text {
                cache_breakpoint, ..
            }
            | Node::ToolCall {
                cache_breakpoint, ..
            }
            | Node::ToolResult {
                cache_breakpoint, ..
            } => Some(cache_breakpoint),
            Node::Image { metadata, .. }
            | Node::Audio { metadata, .. }
            | Node::File { metadata, .. } => Some(&mut metadata.cache_breakpoint),
            Node::Refusal { .. }
            | Node::Reasoning { .. }
            | Node::ProviderItem { .. }
            | Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    pub fn extra_body_mut(&mut self) -> &mut HashMap<String, Value> {
        match self {
            Node::Text { extra_body, .. }
            | Node::Image { extra_body, .. }
            | Node::Audio { extra_body, .. }
            | Node::File { extra_body, .. }
            | Node::Refusal { extra_body, .. }
            | Node::Reasoning { extra_body, .. }
            | Node::ToolCall { extra_body, .. }
            | Node::ProviderItem { extra_body, .. }
            | Node::ToolResult { extra_body, .. }
            | Node::NextDownstreamEnvelopeExtra { extra_body, .. } => extra_body,
        }
    }
    pub fn id(&self) -> Option<&String> {
        match self {
            Node::Text { id, .. }
            | Node::Image { id, .. }
            | Node::Audio { id, .. }
            | Node::File { id, .. }
            | Node::Refusal { id, .. }
            | Node::Reasoning { id, .. }
            | Node::ToolCall { id, .. }
            | Node::ProviderItem { id, .. }
            | Node::ToolResult { id, .. } => id.as_ref(),
            Node::NextDownstreamEnvelopeExtra { .. } => None,
        }
    }

    pub fn set_id(&mut self, new_id: Option<String>) {
        match self {
            Node::Text { id, .. }
            | Node::Image { id, .. }
            | Node::Audio { id, .. }
            | Node::File { id, .. }
            | Node::Refusal { id, .. }
            | Node::Reasoning { id, .. }
            | Node::ToolCall { id, .. }
            | Node::ProviderItem { id, .. }
            | Node::ToolResult { id, .. } => *id = new_id,
            Node::NextDownstreamEnvelopeExtra { .. } => {}
        }
    }
}

pub fn node_is_empty_text(node: &Node) -> bool {
    matches!(node, Node::Text { content, citations, signature, .. } if content.is_empty() && citations.is_empty() && signature.is_none())
}

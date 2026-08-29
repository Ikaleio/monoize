use crate::config::ProviderType;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{
    FILE_ID_ORIGIN_EXTRA_KEY, FILE_ID_ORIGIN_OPENAI, FileSource, ImageSource, Node,
    ToolResultContent, UrpRequest,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashMap;

const BREAKPOINT_KEY: &str = "prompt_cache_breakpoint";
const MAX_IMPLICIT_MODE_EXPLICIT_BREAKPOINTS: usize = 3;
const MAX_EXPLICIT_MODE_BREAKPOINTS: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct CacheOpenAiToolUseTransform;

#[async_trait]
impl Transform for CacheOpenAiToolUseTransform {
    fn type_id(&self) -> &'static str {
        "cache_openai_tool_use"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Auto-cache: OpenAI tool results"),
            ("zh", "自动缓存：OpenAI 工具结果"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Inserts explicit OpenAI prompt-cache breakpoints on eligible tool-result content blocks for explicit-breakpoint GPT models.",
            ),
            (
                "zh",
                "为支持显式缓存断点的 GPT 模型，在符合条件的工具结果内容块上插入显式 prompt-cache 断点。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[
            TransformScope::Provider,
            TransformScope::Global,
            TransformScope::ApiKey,
        ]
    }

    fn config_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
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
        context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        if context.upstream_provider_type != Some(ProviderType::Responses)
            || !supports_explicit_cache_breakpoints(&req.model)
            || !matches!(req.input.last(), Some(Node::ToolResult { .. }))
        {
            return Ok(());
        }

        let trailing_start = req
            .input
            .iter()
            .rposition(|node| !matches!(node, Node::ToolResult { .. }))
            .map_or(0, |idx| idx + 1);
        if trailing_start == 0
            || !matches!(
                req.input.get(trailing_start - 1),
                Some(Node::ToolCall { .. })
            )
        {
            return Ok(());
        }

        let existing_breakpoints = count_explicit_cache_breakpoints(req);
        let mut remaining =
            explicit_cache_breakpoint_limit(req).saturating_sub(existing_breakpoints);
        if remaining == 0 {
            return Ok(());
        }

        let targets = find_eligible_tool_result_targets(req);
        for (node_idx, content_idx) in targets {
            let Node::ToolResult { content, .. } = &mut req.input[node_idx] else {
                unreachable!("target resolution only returns ToolResult nodes");
            };
            let extra_body = content[content_idx].extra_body_mut();
            if extra_body.contains_key(BREAKPOINT_KEY) {
                continue;
            }
            extra_body.insert(BREAKPOINT_KEY.to_string(), json!({"mode": "explicit"}));
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
        Ok(())
    }
}

fn supports_explicit_cache_breakpoints(model: &str) -> bool {
    model
        .split(['/', ':'])
        .filter_map(parse_gpt_version)
        .any(|(major, minor)| major > 5 || (major == 5 && minor >= 6))
}

fn parse_gpt_version(segment: &str) -> Option<(u64, u64)> {
    let version = segment.strip_prefix("gpt-")?;
    let major_len = version.bytes().take_while(u8::is_ascii_digit).count();
    if major_len == 0 {
        return None;
    }
    let major = version[..major_len].parse().ok()?;
    let suffix = &version[major_len..];
    if suffix.is_empty() || suffix.starts_with('-') {
        return Some((major, 0));
    }
    let minor_text = suffix.strip_prefix('.')?;
    let minor_len = minor_text.bytes().take_while(u8::is_ascii_digit).count();
    if minor_len == 0 {
        return None;
    }
    let minor = minor_text[..minor_len].parse().ok()?;
    Some((major, minor))
}

fn explicit_cache_breakpoint_limit(req: &UrpRequest) -> usize {
    if req
        .extra_body
        .get("prompt_cache_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("mode"))
        .and_then(Value::as_str)
        == Some("explicit")
    {
        MAX_EXPLICIT_MODE_BREAKPOINTS
    } else {
        MAX_IMPLICIT_MODE_EXPLICIT_BREAKPOINTS
    }
}

fn count_explicit_cache_breakpoints(req: &UrpRequest) -> usize {
    req.input
        .iter()
        .map(|node| {
            usize::from(node_extra_body(node).contains_key(BREAKPOINT_KEY))
                + match node {
                    Node::ToolResult { content, .. } => content
                        .iter()
                        .filter(|item| {
                            tool_result_content_extra_body(item).contains_key(BREAKPOINT_KEY)
                        })
                        .count(),
                    _ => 0,
                }
        })
        .sum()
}

fn find_eligible_tool_result_targets(req: &UrpRequest) -> Vec<(usize, usize)> {
    let mut targets = Vec::new();
    let mut cursor = req.input.len();
    while cursor > 0 {
        if !matches!(req.input[cursor - 1], Node::ToolResult { .. }) {
            cursor -= 1;
            continue;
        }

        let run_end = cursor;
        let mut run_start = cursor - 1;
        while run_start > 0 && matches!(req.input[run_start - 1], Node::ToolResult { .. }) {
            run_start -= 1;
        }

        if run_start > 0 && matches!(req.input[run_start - 1], Node::ToolCall { .. }) {
            'run: for node_idx in (run_start..run_end).rev() {
                let Node::ToolResult { content, .. } = &req.input[node_idx] else {
                    unreachable!("tool-result run contains only ToolResult nodes");
                };
                for content_idx in (0..content.len()).rev() {
                    if is_eligible_responses_tool_result_content(&content[content_idx]) {
                        targets.push((node_idx, content_idx));
                        break 'run;
                    }
                }
            }
        }
        cursor = run_start;
    }
    targets
}

fn is_eligible_responses_tool_result_content(content: &ToolResultContent) -> bool {
    match content {
        ToolResultContent::Text { .. } => true,
        ToolResultContent::Image {
            source, extra_body, ..
        } => match source {
            ImageSource::Url { .. } | ImageSource::Base64 { .. } => true,
            ImageSource::FileId { .. } => file_id_origin_is_openai(extra_body),
        },
        ToolResultContent::File {
            source, extra_body, ..
        } => match source {
            FileSource::Url { .. } | FileSource::Base64 { .. } => true,
            FileSource::FileId { .. } => file_id_origin_is_openai(extra_body),
            FileSource::Text { .. } | FileSource::Content { .. } => false,
        },
        ToolResultContent::ProviderItem { .. } => false,
    }
}

fn file_id_origin_is_openai(extra_body: &HashMap<String, Value>) -> bool {
    extra_body
        .get(FILE_ID_ORIGIN_EXTRA_KEY)
        .and_then(Value::as_str)
        == Some(FILE_ID_ORIGIN_OPENAI)
}

fn node_extra_body(node: &Node) -> &HashMap<String, Value> {
    match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::Reasoning { extra_body, .. }
        | Node::ToolCall { extra_body, .. }
        | Node::ProviderItem { extra_body, .. }
        | Node::ToolResult { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } => extra_body,
    }
}

fn tool_result_content_extra_body(content: &ToolResultContent) -> &HashMap<String, Value> {
    match content {
        ToolResultContent::Text { extra_body, .. }
        | ToolResultContent::Image { extra_body, .. }
        | ToolResultContent::File { extra_body, .. }
        | ToolResultContent::ProviderItem { extra_body, .. } => extra_body,
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheOpenAiToolUseTransform),
});

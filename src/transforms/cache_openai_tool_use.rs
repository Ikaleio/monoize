use crate::config::ProviderType;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{FileSource, ImageSource, Node, ProviderProtocol, ToolResultContent, UrpRequest};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

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
            || !crate::urp::cache_breakpoint::supports_openai_explicit_breakpoints(&req.model)
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

        let existing_breakpoints = crate::urp::cache_breakpoint::input_breakpoint_count(req);
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
            let Some(slot) = content[content_idx].cache_breakpoint_mut() else {
                unreachable!("eligible tool-result content is a marker carrier");
            };
            if slot.is_some() {
                continue;
            }
            *slot = Some(crate::urp::CacheBreakpoint::openai_responses());
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
        Ok(())
    }
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
            source, metadata, ..
        } => match source {
            ImageSource::Url { .. } | ImageSource::Base64 { .. } => true,
            ImageSource::FileId { .. } => {
                crate::urp::media::resource_matches(metadata, ProviderProtocol::Responses)
            }
        },
        ToolResultContent::File {
            source, metadata, ..
        } => match source {
            FileSource::Url { .. } | FileSource::Base64 { .. } => true,
            FileSource::FileId { .. } => {
                crate::urp::media::resource_matches(metadata, ProviderProtocol::Responses)
            }
            FileSource::Text { .. } | FileSource::Content { .. } => false,
        },
        ToolResultContent::ProviderItem { .. } => false,
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheOpenAiToolUseTransform),
});

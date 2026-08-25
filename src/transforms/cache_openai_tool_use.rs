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
        &[("en", "Auto-cache: OpenAI tool results"), ("zh", "自动缓存：OpenAI 工具结果")]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Inserts explicit OpenAI prompt-cache breakpoints on eligible tool-result content blocks for explicit-breakpoint GPT models."),
            ("zh", "为支持显式缓存断点的 GPT 模型，在符合条件的工具结果内容块上插入显式 prompt-cache 断点。"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::urp::{OrdinaryRole, ToolCallType};
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn context(provider_type: Option<ProviderType>) -> TransformRuntimeContext {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("cache");
        TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: provider_type,
        }
    }

    fn request_with_parallel_tool_results() -> UrpRequest {
        UrpRequest {
            model: "gpt-5.6-sol".to_string(),
            input: vec![
                Node::text(OrdinaryRole::User, "look up both"),
                Node::ToolCall {
                    id: Some("fc_a".to_string()),
                    tool_type: ToolCallType::Function,
                    call_id: "call_a".to_string(),
                    name: "lookup".to_string(),
                    arguments: r#"{"q":"a"}"#.to_string(),
                    extra_body: HashMap::new(),
                },
                Node::ToolCall {
                    id: Some("fc_b".to_string()),
                    tool_type: ToolCallType::Function,
                    call_id: "call_b".to_string(),
                    name: "lookup".to_string(),
                    arguments: r#"{"q":"b"}"#.to_string(),
                    extra_body: HashMap::new(),
                },
                Node::ToolResult {
                    id: Some("fco_a".to_string()),
                    tool_type: ToolCallType::Function,
                    call_id: "call_a".to_string(),
                    is_error: false,
                    content: vec![ToolResultContent::Text {
                        text: "result a".to_string(),
                        extra_body: HashMap::new(),
                    }],
                    extra_body: HashMap::new(),
                },
                Node::ToolResult {
                    id: Some("fco_b".to_string()),
                    tool_type: ToolCallType::Function,
                    call_id: "call_b".to_string(),
                    is_error: false,
                    content: vec![ToolResultContent::Text {
                        text: "result b".to_string(),
                        extra_body: HashMap::new(),
                    }],
                    extra_body: HashMap::new(),
                },
            ],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        }
    }

    fn append_tool_turn(req: &mut UrpRequest, suffix: &str) {
        let call_id = format!("call_{suffix}");
        req.input.push(Node::ToolCall {
            id: Some(format!("fc_{suffix}")),
            tool_type: ToolCallType::Function,
            call_id: call_id.clone(),
            name: "lookup".to_string(),
            arguments: format!(r#"{{"q":"{suffix}"}}"#),
            extra_body: HashMap::new(),
        });
        req.input.push(Node::ToolResult {
            id: Some(format!("fco_{suffix}")),
            tool_type: ToolCallType::Function,
            call_id,
            is_error: false,
            content: vec![ToolResultContent::Text {
                text: format!("result {suffix}"),
                extra_body: HashMap::new(),
            }],
            extra_body: HashMap::new(),
        });
    }

    async fn apply_transform(req: &mut UrpRequest, provider_type: ProviderType) {
        let transform = CacheOpenAiToolUseTransform;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        transform
            .apply(
                UrpData::Request(req),
                Phase::Request,
                &context(Some(provider_type)).await,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");
    }

    fn nested_breakpoint_count(req: &UrpRequest) -> usize {
        count_explicit_cache_breakpoints(req)
    }

    #[test]
    fn recognizes_gpt_5_6_and_later_model_families() {
        assert!(supports_explicit_cache_breakpoints("gpt-5.6"));
        assert!(supports_explicit_cache_breakpoints("gpt-5.6-sol"));
        assert!(supports_explicit_cache_breakpoints("openai/gpt-5.10-pro"));
        assert!(supports_explicit_cache_breakpoints("ft:gpt-6:org:model"));
        assert!(!supports_explicit_cache_breakpoints("gpt-5.5"));
        assert!(!supports_explicit_cache_breakpoints("gpt-5"));
        assert!(!supports_explicit_cache_breakpoints("o4-mini"));
    }

    #[tokio::test]
    async fn marks_latest_tool_result_content_and_encodes_responses_breakpoint() {
        let mut req = request_with_parallel_tool_results();

        apply_transform(&mut req, ProviderType::Responses).await;

        let Node::ToolResult {
            content,
            extra_body,
            ..
        } = &req.input[4]
        else {
            panic!("expected latest tool result");
        };
        assert!(!extra_body.contains_key(BREAKPOINT_KEY));
        assert_eq!(
            tool_result_content_extra_body(&content[0]).get(BREAKPOINT_KEY),
            Some(&json!({"mode": "explicit"}))
        );
        let encoded = crate::urp::encode::openai_responses::encode_request(&req, &req.model);
        let output = encoded["input"]
            .as_array()
            .expect("input array")
            .iter()
            .find(|item| {
                item["type"] == json!("function_call_output") && item["call_id"] == json!("call_b")
            })
            .expect("latest function output");
        assert_eq!(
            output["output"][0][BREAKPOINT_KEY],
            json!({"mode": "explicit"})
        );
        assert!(output.get(BREAKPOINT_KEY).is_none());
    }

    #[tokio::test]
    async fn rebuilds_the_latest_three_tool_result_run_breakpoints() {
        let mut req = request_with_parallel_tool_results();
        append_tool_turn(&mut req, "c");
        append_tool_turn(&mut req, "d");
        append_tool_turn(&mut req, "e");

        apply_transform(&mut req, ProviderType::Responses).await;

        assert_eq!(nested_breakpoint_count(&req), 3);
        for (idx, expected) in [(4, false), (6, true), (8, true), (10, true)] {
            let Node::ToolResult { content, .. } = &req.input[idx] else {
                panic!("expected tool result at index {idx}");
            };
            assert_eq!(
                tool_result_content_extra_body(&content[0]).contains_key(BREAKPOINT_KEY),
                expected,
                "unexpected breakpoint state at index {idx}"
            );
        }
    }

    #[tokio::test]
    async fn is_idempotent_and_preserves_existing_breakpoint() {
        let mut req = request_with_parallel_tool_results();
        let Node::ToolResult { content, .. } = &mut req.input[4] else {
            panic!("expected latest tool result");
        };
        content[0]
            .extra_body_mut()
            .insert(BREAKPOINT_KEY.to_string(), json!({"mode": "client"}));

        apply_transform(&mut req, ProviderType::Responses).await;
        apply_transform(&mut req, ProviderType::Responses).await;

        assert_eq!(nested_breakpoint_count(&req), 1);
        let Node::ToolResult { content, .. } = &req.input[4] else {
            panic!("expected latest tool result");
        };
        assert_eq!(
            tool_result_content_extra_body(&content[0]).get(BREAKPOINT_KEY),
            Some(&json!({"mode": "client"}))
        );
    }

    #[tokio::test]
    async fn respects_implicit_and_explicit_mode_write_limits() {
        let mut implicit = request_with_parallel_tool_results();
        for node in implicit.input.iter_mut().take(3) {
            node.extra_body_mut()
                .insert(BREAKPOINT_KEY.to_string(), json!({"mode": "explicit"}));
        }

        apply_transform(&mut implicit, ProviderType::Responses).await;
        assert_eq!(nested_breakpoint_count(&implicit), 3);

        let mut explicit = implicit;
        explicit.extra_body.insert(
            "prompt_cache_options".to_string(),
            json!({"mode": "explicit", "ttl": "30m"}),
        );
        apply_transform(&mut explicit, ProviderType::Responses).await;
        assert_eq!(nested_breakpoint_count(&explicit), 4);
        assert_eq!(
            explicit.extra_body.get("prompt_cache_options"),
            Some(&json!({"mode": "explicit", "ttl": "30m"}))
        );
    }

    #[tokio::test]
    async fn rejects_unsupported_provider_model_and_tool_shape() {
        let mut chat = request_with_parallel_tool_results();
        apply_transform(&mut chat, ProviderType::ChatCompletion).await;
        assert_eq!(nested_breakpoint_count(&chat), 0);

        let mut old_model = request_with_parallel_tool_results();
        old_model.model = "gpt-5.5".to_string();
        apply_transform(&mut old_model, ProviderType::Responses).await;
        assert_eq!(nested_breakpoint_count(&old_model), 0);

        let mut no_tool_call = request_with_parallel_tool_results();
        no_tool_call.input.drain(1..3);
        apply_transform(&mut no_tool_call, ProviderType::Responses).await;
        assert_eq!(nested_breakpoint_count(&no_tool_call), 0);
    }
}

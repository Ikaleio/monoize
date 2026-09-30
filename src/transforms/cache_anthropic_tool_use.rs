use crate::transforms::anthropic_cache::CacheConfig;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{AnthropicCacheTarget, Node};
use async_trait::async_trait;
use serde_json::Value;

pub struct CacheAnthropicToolUseTransform;

/// Mark the last tool result so the cache point follows a growing tool loop.
/// Respects the max-4 cache breakpoint limit.
#[async_trait]
impl Transform for CacheAnthropicToolUseTransform {
    fn type_id(&self) -> &'static str {
        "cache_anthropic_tool_use"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Auto-cache: Anthropic tool results"),
            ("zh", "自动缓存：Anthropic 工具结果"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "On tool-result submissions, inserts an Anthropic ephemeral cache_control breakpoint on the final tool result.",
            ),
            (
                "zh",
                "在提交工具结果时，于最后一个工具结果插入 Anthropic ephemeral cache_control 缓存断点。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request]
    }

    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[TransformScope::Provider, TransformScope::ApiKey]
    }

    fn config_schema(&self) -> Value {
        CacheConfig::schema()
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        CacheConfig::parse(raw)
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(NoState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        _phase: Phase,
        _context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        if !matches!(req.input.last(), Some(Node::ToolResult { .. })) {
            return Ok(());
        }

        let slot_free = count_cache_breakpoints(req) < 4;
        let last_node = req.input.last_mut().expect("checked trailing tool result");
        CacheConfig::from_dyn(config)?.apply_marker(
            last_node.extra_body_mut(),
            &mut req.context.anthropic_cache_markers,
            AnthropicCacheTarget::LastToolResult,
            slot_free,
        );

        Ok(())
    }
}

fn count_cache_breakpoints(req: &crate::urp::UrpRequest) -> usize {
    req.input
        .iter()
        .filter(|node| node_has_cache_control(node))
        .count()
        + usize::from(req.extra_body.contains_key("cache_control"))
}

fn node_has_cache_control(node: &Node) -> bool {
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
        | Node::NextDownstreamEnvelopeExtra { extra_body } => {
            extra_body.contains_key("cache_control")
        }
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheAnthropicToolUseTransform),
});

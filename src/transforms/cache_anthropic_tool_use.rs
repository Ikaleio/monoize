use crate::transforms::anthropic_cache::CacheConfig;
use crate::transforms::anthropic_cache::anthropic_slot_count;
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

        let slot_free = anthropic_slot_count(req) < 4;
        let slot = req
            .input
            .last_mut()
            .and_then(Node::cache_breakpoint_mut)
            .expect("checked trailing tool result");
        CacheConfig::from_dyn(config)?.apply_breakpoint(
            slot,
            &mut req.context.anthropic_cache_markers,
            AnthropicCacheTarget::LastToolResult,
            slot_free,
        );

        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheAnthropicToolUseTransform),
});

use crate::transforms::anthropic_cache::CacheConfig;
use crate::transforms::anthropic_cache::anthropic_slot_count;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, OrdinaryRole};
use async_trait::async_trait;
use serde_json::Value;

pub struct CacheAnthropicSystemTransform;

/// Marks the last system or developer carrier unless the client already marked it.
/// Respects the max-4 cache breakpoint limit.
#[async_trait]
impl Transform for CacheAnthropicSystemTransform {
    fn type_id(&self) -> &'static str {
        "cache_anthropic_system"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Auto-cache: Anthropic system prompt"),
            ("zh", "自动缓存：Anthropic 系统提示词"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Inserts an Anthropic ephemeral cache_control breakpoint on the last system or developer node so the stable system prefix can be cached.",
            ),
            (
                "zh",
                "在最后一个 system/developer 节点插入 Anthropic ephemeral cache_control 缓存断点，使稳定的系统前缀可以被缓存。",
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

        let system_idx = req.input.iter_mut().rposition(|node| {
            matches!(
                node.role(),
                Some(OrdinaryRole::System | OrdinaryRole::Developer)
            ) && !matches!(node, Node::Text { content, .. } if content.is_empty())
                && node.cache_breakpoint_mut().is_some()
        });
        let Some(idx) = system_idx else {
            return Ok(());
        };

        let slot_free = anthropic_slot_count(req) < 4;
        let slot = req.input[idx]
            .cache_breakpoint_mut()
            .expect("checked marker carrier");
        CacheConfig::from_dyn(config)?.apply_breakpoint(slot, slot_free);

        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheAnthropicSystemTransform),
});

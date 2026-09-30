use crate::transforms::anthropic_cache::CacheConfig;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{AnthropicCacheTarget, Node, OrdinaryRole};
use async_trait::async_trait;
use serde_json::Value;

pub struct CacheAnthropicSystemTransform;

/// If the system prompt has no cache_control on any of its parts,
/// add cache_control: {type: "ephemeral"} to its last part.
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

        let system_idx = req.input.iter().rposition(|node| {
            matches!(
                node.role(),
                Some(OrdinaryRole::System | OrdinaryRole::Developer)
            )
        });
        let Some(idx) = system_idx else {
            return Ok(());
        };

        let slot_free = count_cache_breakpoints(req) < 4;
        CacheConfig::from_dyn(config)?.apply_marker(
            req.input[idx].extra_body_mut(),
            &mut req.context.anthropic_cache_markers,
            AnthropicCacheTarget::System,
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
    factory: || Box::new(CacheAnthropicSystemTransform),
});

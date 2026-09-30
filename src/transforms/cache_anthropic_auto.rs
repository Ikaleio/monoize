use crate::config::ProviderType;
use crate::transforms::anthropic_cache::CacheConfig;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{AnthropicCacheTarget, Node};
use async_trait::async_trait;
use serde_json::Value;

pub struct CacheAnthropicAutoTransform;

#[async_trait]
impl Transform for CacheAnthropicAutoTransform {
    fn type_id(&self) -> &'static str {
        "cache_anthropic_auto"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Auto-cache: Anthropic request"),
            ("zh", "自动缓存：Anthropic 请求"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Adds top-level cache_control to Anthropic Messages requests so the cache point follows the last eligible block.",
            ),
            (
                "zh",
                "向 Anthropic Messages 请求顶层添加 cache_control，使缓存断点跟随最后一个可缓存块。",
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
        context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };
        if context.upstream_provider_type != Some(ProviderType::Messages) {
            return Ok(());
        }

        let explicit_breakpoints = req
            .input
            .iter()
            .filter(|node| match node {
                Node::Text { extra_body, .. }
                | Node::Image { extra_body, .. }
                | Node::Audio { extra_body, .. }
                | Node::File { extra_body, .. }
                | Node::Refusal { extra_body, .. }
                | Node::Reasoning { extra_body, .. }
                | Node::ToolCall { extra_body, .. }
                | Node::ToolResult { extra_body, .. }
                | Node::ProviderItem { extra_body, .. }
                | Node::NextDownstreamEnvelopeExtra { extra_body } => {
                    extra_body.contains_key("cache_control")
                }
            })
            .count();
        CacheConfig::from_dyn(config)?.apply_marker(
            &mut req.extra_body,
            &mut req.context.anthropic_cache_markers,
            AnthropicCacheTarget::Request,
            explicit_breakpoints < 4,
        );
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheAnthropicAutoTransform),
});

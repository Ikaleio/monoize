use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, OrdinaryRole};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;

const HEADER_PREFIX: &str = "x-anthropic-billing-header:";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct PromptStripAnthropicBillingHeaderTransform;

#[async_trait]
impl Transform for PromptStripAnthropicBillingHeaderTransform {
    fn type_id(&self) -> &'static str {
        "prompt_strip_anthropic_billing_header"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Prompt: strip Anthropic billing header"),
            ("zh", "提示词：移除 Anthropic 计费标记"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Removes x-anthropic-billing-header lines from system and developer text nodes, dropping nodes that become empty.",
            ),
            (
                "zh",
                "从 system/developer 文本节点中移除 x-anthropic-billing-header 行，并删除因此变空的节点。",
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
        _context: &TransformRuntimeContext,
        _config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };

        req.input.retain_mut(|node| match node {
            Node::Text { role, content, .. }
                if matches!(role, OrdinaryRole::System | OrdinaryRole::Developer) =>
            {
                strip_header_lines(content);
                !content.is_empty()
            }
            _ => true,
        });

        Ok(())
    }
}

fn strip_header_lines(content: &mut String) {
    let mut out = String::new();
    for segment in content.split_inclusive('\n') {
        let line = segment.trim_end_matches('\n').trim_end_matches('\r');
        if line.trim_start().starts_with(HEADER_PREFIX) {
            continue;
        }
        out.push_str(segment);
    }
    if !content.ends_with('\n') && content.lines().count() <= 1 {
        let line = content.trim_end_matches('\r');
        if line.trim_start().starts_with(HEADER_PREFIX) {
            out.clear();
        }
    }
    *content = out.trim_matches('\n').trim_end_matches('\r').to_string();
}

inventory::submit!(TransformEntry {
    factory: || Box::new(PromptStripAnthropicBillingHeaderTransform),
});

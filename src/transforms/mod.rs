use crate::urp::{Node, OrdinaryRole, UrpRequest, UrpResponse, UrpStreamEvent};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub mod cache_anthropic_system;
pub mod cache_anthropic_tool_use;
pub mod cache_openai_prompt;
pub mod cache_openai_tool_use;
pub mod cache_user_id;
pub mod field_override_max_tokens;
pub mod field_remove;
pub mod field_set;
pub mod image_compress;
pub mod image_enable_openai_generation_tool;
pub mod image_markdown_to_output;
pub mod image_output_to_markdown;
pub mod image_resolve_urls;
pub mod prompt_append_empty_user;
pub mod prompt_inject_system;
pub mod prompt_strip_anthropic_billing_header;
pub mod prompt_strip_orphaned_tool_calls;
pub mod reasoning_content_to_summary;
pub mod reasoning_effort_to_budget;
pub mod reasoning_effort_to_model_suffix;
pub mod reasoning_from_think_xml;
pub mod reasoning_inject_content_field;
pub mod reasoning_strip_encrypted;
pub mod reasoning_strip_input;
pub mod reasoning_strip_output;
pub mod reasoning_summary_to_raw_cot;
pub mod reasoning_to_think_xml;
pub mod role_developer_to_system;
pub mod role_merge_consecutive;
pub mod role_system_to_developer;
pub mod stream_force;
pub mod stream_split_sse_frames;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformScope {
    Provider,
    Global,
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformRuleConfig {
    pub transform: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub models: Option<Vec<String>>,
    pub phase: Phase,
    #[serde(default)]
    pub config: Value,
}

fn default_enabled() -> bool {
    true
}

/// TF-17 canonicalization map: exactly the historical IDs listed in
/// `spec/urp-transform-system.spec.md` map to canonical IDs; everything else
/// (including already-canonical IDs and unknown IDs) is returned unchanged.
pub const HISTORICAL_TRANSFORM_ID_MAP: &[(&str, &str)] = &[
    ("append_empty_user_message", "prompt_append_empty_user"),
    ("assistant_markdown_images_to_output", "image_markdown_to_output"),
    ("assistant_output_images_to_markdown", "image_output_to_markdown"),
    ("auto_cache_openai", "cache_openai_prompt"),
    ("auto_cache_openai_prompt", "cache_openai_prompt"),
    ("auto_cache_openai_prompt_key", "cache_openai_prompt"),
    ("auto_cache_openai_tool_use", "cache_openai_tool_use"),
    ("auto_cache_system", "cache_anthropic_system"),
    ("auto_cache_tool_use", "cache_anthropic_tool_use"),
    ("auto_cache_user_id", "cache_user_id"),
    ("compress_assistant_output_images", "image_compress_output"),
    ("compress_user_message_images", "image_compress_input"),
    ("developer_to_system_role", "role_developer_to_system"),
    (
        "enable_openai_image_generation_tool",
        "image_enable_openai_generation_tool",
    ),
    ("force_stream", "stream_force"),
    ("inject_system_prompt", "prompt_inject_system"),
    ("merge_consecutive_roles", "role_merge_consecutive"),
    ("openai_prompt_cache", "cache_openai_prompt"),
    ("override_max_tokens", "field_override_max_tokens"),
    ("plaintext_reasoning_to_summary", "reasoning_content_to_summary"),
    ("reasoning_content_delta", "reasoning_inject_content_field"),
    (
        "remove_anthropic_billing_header",
        "prompt_strip_anthropic_billing_header",
    ),
    (
        "remove_anthropic_billing_headers",
        "prompt_strip_anthropic_billing_header",
    ),
    ("remove_field", "field_remove"),
    ("set_field", "field_set"),
    ("split_sse_frames", "stream_split_sse_frames"),
    (
        "strip_anthropic_billing_header",
        "prompt_strip_anthropic_billing_header",
    ),
    (
        "strip_anthropic_billing_headers",
        "prompt_strip_anthropic_billing_header",
    ),
    (
        "strip_claude_code_billing_header",
        "prompt_strip_anthropic_billing_header",
    ),
    ("strip_encrypted_reasoning", "reasoning_strip_encrypted"),
    ("strip_input_reasoning", "reasoning_strip_input"),
    ("strip_orphaned_tool_use", "prompt_strip_orphaned_tool_calls"),
    ("strip_reasoning", "reasoning_strip_output"),
    ("system_to_developer_role", "role_system_to_developer"),
    ("think_xml_to_reasoning", "reasoning_from_think_xml"),
];

pub fn canonical_transform_id(transform: &str) -> &str {
    HISTORICAL_TRANSFORM_ID_MAP
        .iter()
        .find(|(historical, _)| *historical == transform)
        .map(|(_, canonical)| *canonical)
        .unwrap_or(transform)
}

pub fn canonicalize_transform_rule(rule: &mut TransformRuleConfig) -> bool {
    let canonical = canonical_transform_id(&rule.transform);
    if canonical == rule.transform {
        return false;
    }
    rule.transform = canonical.to_string();
    true
}

pub fn canonicalize_transform_rules(rules: &mut [TransformRuleConfig]) -> bool {
    // `Iterator::any` would short-circuit on the first rewritten rule and leave
    // later stale IDs untouched, so every rule must be visited unconditionally.
    let mut changed = false;
    for rule in rules.iter_mut() {
        changed |= canonicalize_transform_rule(rule);
    }
    changed
}

pub enum UrpData<'a> {
    Request(&'a mut UrpRequest),
    Response(&'a mut UrpResponse),
    Stream(&'a mut UrpStreamEvent),
}

impl<'a> UrpData<'a> {
    pub fn reborrow(&mut self) -> UrpData<'_> {
        match self {
            Self::Request(v) => UrpData::Request(v),
            Self::Response(v) => UrpData::Response(v),
            Self::Stream(v) => UrpData::Stream(v),
        }
    }
}

pub trait TransformConfig: Send + Sync + 'static {
    fn as_any(&self) -> &dyn Any;
}

pub trait TransformState: Send + Sync {
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn finalize_stream_event(&mut self, event: UrpStreamEvent) -> Vec<UrpStreamEvent> {
        vec![event]
    }
}

pub struct NoState;

impl TransformState for NoState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct TransformRuntimeContext {
    pub image_transform_cache: Arc<crate::image_transform_cache::ImageTransformCache>,
    pub http_client: reqwest::Client,
    pub upstream_provider_type: Option<crate::config::ProviderType>,
}

/// Localized display metadata entries as `(language, text)` pairs.
/// TF-8a requires at least `en` and `zh` with non-empty text.
pub type LocalizedText = &'static [(&'static str, &'static str)];

#[async_trait]
pub trait Transform: Send + Sync + 'static {
    fn type_id(&self) -> &'static str;
    /// Localized human-readable name per TF-1b / TF-8a.
    fn display_name(&self) -> LocalizedText;
    /// Localized human-readable description per TF-1b / TF-8a.
    fn display_description(&self) -> LocalizedText;
    fn supported_phases(&self) -> &'static [Phase];
    fn supported_scopes(&self) -> &'static [TransformScope] {
        &[TransformScope::Provider]
    }
    fn config_schema(&self) -> Value;
    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError>;
    fn init_state(&self) -> Box<dyn TransformState>;
    async fn apply(
        &self,
        data: UrpData<'_>,
        phase: Phase,
        context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError>;
}

#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("transform not found: {0}")]
    NotFound(String),
    #[error("transform apply failed: {0}")]
    Apply(String),
}

/// Fixed namespace prefix for dynamically registered custom transforms
/// (CJS-ID-1). Built-in canonical IDs can never collide with it because TF-14
/// forbids `:` in canonical IDs.
pub const CUSTOM_TRANSFORM_ID_PREFIX: &str = "js:";

/// A dynamically registered transform executor (custom `js:` transforms).
/// Unlike [`Transform`], display metadata lives outside this trait; the
/// pipeline only needs declared phases, state construction, and application.
/// The raw rule `config` value is passed through unparsed (CJS-JS-1).
#[async_trait]
pub trait DynTransform: Send + Sync {
    fn declared_phases(&self) -> &[Phase];
    fn init_state(&self) -> Box<dyn TransformState>;
    async fn apply(
        &self,
        data: UrpData<'_>,
        phase: Phase,
        context: &TransformRuntimeContext,
        config: &Value,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError>;
}

/// Resolves enabled `js:` transforms at execution time. Implemented by the
/// custom-transform snapshot; kept as a trait so this module stays free of a
/// dependency on the custom-transform store.
pub trait CustomTransformSource: Send + Sync {
    fn resolve_custom(&self, id: &str) -> Option<Arc<dyn DynTransform>>;
}

/// CJS-RT-2 lookup: built-in registry first, then the custom snapshot for
/// `js:`-prefixed IDs. Constructed per apply call; both borrows must outlive
/// the pipeline invocation.
#[derive(Clone, Copy)]
pub struct TransformResolver<'a> {
    builtin: &'a TransformRegistry,
    custom: Option<&'a dyn CustomTransformSource>,
}

enum ResolvedTransform {
    Builtin(Arc<dyn Transform>),
    Custom(Arc<dyn DynTransform>),
    /// CJS-RT-3: a `js:` rule whose transform is deleted or disabled is a
    /// silent no-op instead of a request failure.
    SkippedCustom,
}

impl<'a> TransformResolver<'a> {
    pub fn new(builtin: &'a TransformRegistry, custom: &'a dyn CustomTransformSource) -> Self {
        Self {
            builtin,
            custom: Some(custom),
        }
    }

    fn resolve(&self, raw_id: &str) -> Result<ResolvedTransform, TransformError> {
        let id = canonical_transform_id(raw_id);
        if let Some(transform) = self.builtin.get(id) {
            return Ok(ResolvedTransform::Builtin(transform.clone()));
        }
        if id.starts_with(CUSTOM_TRANSFORM_ID_PREFIX) {
            if let Some(custom) = self.custom.and_then(|source| source.resolve_custom(id)) {
                return Ok(ResolvedTransform::Custom(custom));
            }
            tracing::warn!(transform_id = id, "skipping unresolved custom transform rule");
            return Ok(ResolvedTransform::SkippedCustom);
        }
        Err(TransformError::NotFound(id.to_string()))
    }
}

impl<'a> From<&'a TransformRegistry> for TransformResolver<'a> {
    fn from(builtin: &'a TransformRegistry) -> Self {
        Self {
            builtin,
            custom: None,
        }
    }
}

pub struct TransformEntry {
    pub factory: fn() -> Box<dyn Transform>,
}

inventory::collect!(TransformEntry);

pub type TransformRegistry = HashMap<&'static str, Arc<dyn Transform>>;

fn builtin_transforms() -> Vec<Box<dyn Transform>> {
    vec![
        Box::new(cache_anthropic_system::CacheAnthropicSystemTransform),
        Box::new(cache_anthropic_tool_use::CacheAnthropicToolUseTransform),
        Box::new(cache_openai_prompt::CacheOpenAiPromptTransform),
        Box::new(cache_openai_tool_use::CacheOpenAiToolUseTransform),
        Box::new(cache_user_id::CacheUserIdTransform),
        Box::new(field_override_max_tokens::FieldOverrideMaxTokensTransform),
        Box::new(field_remove::FieldRemoveTransform),
        Box::new(field_set::FieldSetTransform),
        Box::new(image_compress::ImageCompressInputTransform),
        Box::new(image_compress::ImageCompressOutputTransform),
        Box::new(image_enable_openai_generation_tool::ImageEnableOpenAiGenerationToolTransform),
        Box::new(image_markdown_to_output::ImageMarkdownToOutputTransform),
        Box::new(image_output_to_markdown::ImageOutputToMarkdownTransform),
        Box::new(image_resolve_urls::ImageResolveUrlsTransform),
        Box::new(prompt_append_empty_user::PromptAppendEmptyUserTransform),
        Box::new(prompt_inject_system::PromptInjectSystemTransform),
        Box::new(prompt_strip_anthropic_billing_header::PromptStripAnthropicBillingHeaderTransform),
        Box::new(prompt_strip_orphaned_tool_calls::PromptStripOrphanedToolCallsTransform),
        Box::new(reasoning_content_to_summary::ReasoningContentToSummaryTransform),
        Box::new(reasoning_effort_to_budget::ReasoningEffortToBudgetTransform),
        Box::new(reasoning_effort_to_model_suffix::ReasoningEffortToModelSuffixTransform),
        Box::new(reasoning_from_think_xml::ReasoningFromThinkXmlTransform),
        Box::new(reasoning_inject_content_field::ReasoningInjectContentFieldTransform),
        Box::new(reasoning_strip_encrypted::ReasoningStripEncryptedTransform),
        Box::new(reasoning_strip_input::ReasoningStripInputTransform),
        Box::new(reasoning_strip_output::ReasoningStripOutputTransform),
        Box::new(reasoning_summary_to_raw_cot::ReasoningSummaryToRawCotTransform),
        Box::new(reasoning_to_think_xml::ReasoningToThinkXmlTransform),
        Box::new(role_developer_to_system::RoleDeveloperToSystemTransform),
        Box::new(role_merge_consecutive::RoleMergeConsecutiveTransform),
        Box::new(role_system_to_developer::RoleSystemToDeveloperTransform),
        Box::new(stream_force::StreamForceTransform),
        Box::new(stream_split_sse_frames::StreamSplitSseFramesTransform),
    ]
}

pub fn registry() -> TransformRegistry {
    let mut map = HashMap::new();
    for transform in builtin_transforms() {
        let type_id = Transform::type_id(&*transform);
        map.insert(type_id, Arc::<dyn Transform>::from(transform));
    }
    for entry in inventory::iter::<TransformEntry> {
        let transform = (entry.factory)();
        let type_id = Transform::type_id(&*transform);
        map.insert(type_id, Arc::<dyn Transform>::from(transform));
    }
    map
}

pub fn build_states_for_rules<'a>(
    rules: &[TransformRuleConfig],
    resolver: impl Into<TransformResolver<'a>>,
) -> Result<Vec<Box<dyn TransformState>>, TransformError> {
    let resolver = resolver.into();
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        match resolver.resolve(rule.transform.as_str())? {
            ResolvedTransform::Builtin(transform) => out.push(transform.init_state()),
            ResolvedTransform::Custom(transform) => out.push(transform.init_state()),
            ResolvedTransform::SkippedCustom => out.push(Box::new(NoState)),
        }
    }
    Ok(out)
}

pub async fn apply_transforms<'a>(
    mut data: UrpData<'_>,
    rules: &[TransformRuleConfig],
    states: &mut [Box<dyn TransformState>],
    current_model: &str,
    phase: Phase,
    context: &TransformRuntimeContext,
    resolver: impl Into<TransformResolver<'a>>,
) -> Result<(), TransformError> {
    let resolver = resolver.into();
    if rules.len() != states.len() {
        return Err(TransformError::Apply(
            "rule/state length mismatch".to_string(),
        ));
    }
    for (i, rule) in rules.iter().enumerate() {
        if !rule.enabled || rule.phase != phase {
            continue;
        }
        if let Some(patterns) = &rule.models {
            if !patterns
                .iter()
                .any(|pattern| model_glob_match(pattern, current_model))
            {
                continue;
            }
        }
        match resolver.resolve(rule.transform.as_str())? {
            ResolvedTransform::SkippedCustom => continue,
            ResolvedTransform::Builtin(transform) => {
                let config = transform.parse_config(rule.config.clone())?;
                transform
                    .apply(
                        data.reborrow(),
                        phase,
                        context,
                        config.as_ref(),
                        states[i].as_mut(),
                    )
                    .await?;
            }
            ResolvedTransform::Custom(transform) => {
                // CJS-RT-5: a rule phase outside the declared phases is a no-op.
                if !transform.declared_phases().contains(&phase) {
                    continue;
                }
                transform
                    .apply(
                        data.reborrow(),
                        phase,
                        context,
                        &rule.config,
                        states[i].as_mut(),
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

pub async fn apply_stream_transforms<'a>(
    initial_event: UrpStreamEvent,
    rules: &[TransformRuleConfig],
    states: &mut [Box<dyn TransformState>],
    current_model: &str,
    phase: Phase,
    context: &TransformRuntimeContext,
    resolver: impl Into<TransformResolver<'a>>,
) -> Result<Vec<UrpStreamEvent>, TransformError> {
    let resolver = resolver.into();
    if rules.len() != states.len() {
        return Err(TransformError::Apply(
            "rule/state length mismatch".to_string(),
        ));
    }

    let mut events = vec![initial_event];
    for (i, rule) in rules.iter().enumerate() {
        if !rule.enabled || rule.phase != phase {
            continue;
        }
        if let Some(patterns) = &rule.models {
            if !patterns
                .iter()
                .any(|pattern| model_glob_match(pattern, current_model))
            {
                continue;
            }
        }
        enum StreamStep {
            Builtin(Arc<dyn Transform>, Box<dyn TransformConfig>),
            Custom(Arc<dyn DynTransform>),
        }
        let step = match resolver.resolve(rule.transform.as_str())? {
            ResolvedTransform::SkippedCustom => continue,
            ResolvedTransform::Builtin(transform) => {
                let config = transform.parse_config(rule.config.clone())?;
                StreamStep::Builtin(transform, config)
            }
            ResolvedTransform::Custom(transform) => {
                if !transform.declared_phases().contains(&phase) {
                    continue;
                }
                StreamStep::Custom(transform)
            }
        };
        let mut next_events = Vec::new();
        for mut event in events {
            match &step {
                StreamStep::Builtin(transform, config) => {
                    transform
                        .apply(
                            UrpData::Stream(&mut event),
                            phase,
                            context,
                            config.as_ref(),
                            states[i].as_mut(),
                        )
                        .await?;
                }
                StreamStep::Custom(transform) => {
                    transform
                        .apply(
                            UrpData::Stream(&mut event),
                            phase,
                            context,
                            &rule.config,
                            states[i].as_mut(),
                        )
                        .await?;
                }
            }
            next_events.extend(states[i].finalize_stream_event(event));
        }
        events = next_events;
    }

    Ok(events)
}

pub fn model_glob_match(pattern: &str, model: &str) -> bool {
    crate::glob::case_sensitive_glob_match(pattern, model)
}

pub fn text_node(role: OrdinaryRole, content: impl Into<String>) -> Node {
    Node::Text {
        id: None,
        role,
        content: content.into(),
        phase: None,
        extra_body: HashMap::new(),
    }
}

pub fn move_system_to_developer_nodes(nodes: &mut [Node]) {
    for node in nodes.iter_mut() {
        match node {
            Node::Text { role, .. }
            | Node::Image { role, .. }
            | Node::Audio { role, .. }
            | Node::File { role, .. }
            | Node::ProviderItem { role, .. } => {
                if *role == OrdinaryRole::System {
                    *role = OrdinaryRole::Developer;
                }
            }
            _ => {}
        }
    }
}

pub fn move_developer_to_system_nodes(nodes: &mut [Node]) {
    for node in nodes.iter_mut() {
        match node {
            Node::Text { role, .. }
            | Node::Image { role, .. }
            | Node::Audio { role, .. }
            | Node::File { role, .. }
            | Node::ProviderItem { role, .. } => {
                if *role == OrdinaryRole::Developer {
                    *role = OrdinaryRole::System;
                }
            }
            _ => {}
        }
    }
}

pub fn strip_reasoning_nodes(nodes: &[Node]) -> Vec<Node> {
    nodes
        .iter()
        .filter(|node| !matches!(node, Node::Reasoning { .. }))
        .cloned()
        .collect()
}

pub fn set_extra_path(extra: &mut HashMap<String, Value>, path: &str, value: Value) {
    let keys: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if keys.is_empty() {
        return;
    }
    if keys.len() == 1 {
        extra.insert(keys[0].to_string(), value);
        return;
    }

    let first = keys[0].to_string();
    if !extra.contains_key(&first) || !extra[&first].is_object() {
        extra.insert(first.clone(), Value::Object(Map::new()));
    }
    let Some(mut cursor) = extra.get_mut(&first) else {
        return;
    };
    for key in keys.iter().skip(1).take(keys.len().saturating_sub(2)) {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        let Some(obj) = cursor.as_object_mut() else {
            return;
        };
        let entry = obj
            .entry((*key).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        cursor = entry;
    }
    if let Some(last_key) = keys.last() {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        if let Some(obj) = cursor.as_object_mut() {
            obj.insert((*last_key).to_string(), value);
        }
    }
}

pub fn remove_extra_path(extra: &mut HashMap<String, Value>, path: &str) {
    let keys: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if keys.is_empty() {
        return;
    }
    if keys.len() == 1 {
        extra.remove(keys[0]);
        return;
    }
    let first = keys[0];
    let Some(mut current) = extra.get_mut(first) else {
        return;
    };
    for key in keys.iter().skip(1).take(keys.len().saturating_sub(2)) {
        let Some(obj) = current.as_object_mut() else {
            return;
        };
        let Some(next) = obj.get_mut(*key) else {
            return;
        };
        current = next;
    }
    let Some(obj) = current.as_object_mut() else {
        return;
    };
    if let Some(last) = keys.last() {
        obj.remove(*last);
    }
}

pub fn state_set_insert(state: &mut dyn TransformState, key: u32) {
    if let Some(set) = state.as_any_mut().downcast_mut::<HashSet<u32>>() {
        set.insert(key);
    }
}

pub fn state_set_contains(state: &mut dyn TransformState, key: u32) -> bool {
    if let Some(set) = state.as_any_mut().downcast_mut::<HashSet<u32>>() {
        return set.contains(&key);
    }
    false
}

#[cfg(test)]
mod registry_tests {
    use super::{
        HISTORICAL_TRANSFORM_ID_MAP, TransformRuleConfig, canonical_transform_id,
        canonicalize_transform_rule, canonicalize_transform_rules, registry,
    };

    /// TF-7a domain vocabulary.
    const TRANSFORM_ID_DOMAINS: &[&str] = &[
        "cache",
        "field",
        "image",
        "prompt",
        "reasoning",
        "role",
        "stream",
    ];

    /// TF-7 canonical built-in list.
    const EXPECTED_BUILTIN_IDS: &[&str] = &[
        "cache_anthropic_system",
        "cache_anthropic_tool_use",
        "cache_openai_prompt",
        "cache_openai_tool_use",
        "cache_user_id",
        "field_override_max_tokens",
        "field_remove",
        "field_set",
        "image_compress_input",
        "image_compress_output",
        "image_enable_openai_generation_tool",
        "image_markdown_to_output",
        "image_output_to_markdown",
        "image_resolve_urls",
        "prompt_append_empty_user",
        "prompt_inject_system",
        "prompt_strip_anthropic_billing_header",
        "prompt_strip_orphaned_tool_calls",
        "reasoning_content_to_summary",
        "reasoning_effort_to_budget",
        "reasoning_effort_to_model_suffix",
        "reasoning_from_think_xml",
        "reasoning_inject_content_field",
        "reasoning_strip_encrypted",
        "reasoning_strip_input",
        "reasoning_strip_output",
        "reasoning_summary_to_raw_cot",
        "reasoning_to_think_xml",
        "role_developer_to_system",
        "role_merge_consecutive",
        "role_system_to_developer",
        "stream_force",
        "stream_split_sse_frames",
    ];

    #[test]
    fn registry_contains_exactly_the_tf7_builtin_ids() {
        let registry = registry();
        let mut ids: Vec<&str> = registry.keys().copied().collect();
        ids.sort_unstable();
        assert_eq!(ids, EXPECTED_BUILTIN_IDS);
    }

    #[test]
    fn registry_contains_reasoning_inject_content_field_and_api_key_scope_metadata() {
        let registry = registry();
        let transform = registry
            .get("reasoning_inject_content_field")
            .expect("reasoning_inject_content_field should be registered");

        assert!(
            transform
                .supported_phases()
                .iter()
                .any(|phase| matches!(phase, super::Phase::Response))
        );
        assert!(
            transform
                .supported_scopes()
                .iter()
                .any(|scope| matches!(scope, super::TransformScope::ApiKey))
        );
    }

    #[test]
    fn global_scope_serializes_as_global() {
        assert_eq!(
            serde_json::to_value(super::TransformScope::Global).expect("scope serializes"),
            serde_json::json!("global")
        );
    }

    #[test]
    fn canonical_transform_ids_are_lower_snake_case_with_tf7a_domain_prefix() {
        let registry = registry();
        for transform_id in registry.keys() {
            assert!(
                transform_id
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
                "transform id {transform_id} must be lowercase snake_case"
            );
            let domain = transform_id.split('_').next().unwrap_or_default();
            assert!(
                TRANSFORM_ID_DOMAINS.contains(&domain),
                "transform id {transform_id} must start with a TF-7a domain segment"
            );
        }
    }

    #[test]
    fn every_registry_transform_has_en_and_zh_display_metadata() {
        let registry = registry();
        for (transform_id, transform) in registry.iter() {
            for (kind, entries) in [
                ("name", transform.display_name()),
                ("description", transform.display_description()),
            ] {
                for locale in ["en", "zh"] {
                    let text = entries
                        .iter()
                        .find(|(language, _)| *language == locale)
                        .map(|(_, text)| *text);
                    assert!(
                        text.is_some_and(|text| !text.trim().is_empty()),
                        "transform {transform_id} must define a non-empty {locale} {kind}"
                    );
                }
            }
        }
    }

    #[test]
    fn historical_map_targets_registered_canonical_ids_only() {
        let registry = registry();
        for (historical, canonical) in HISTORICAL_TRANSFORM_ID_MAP {
            assert!(
                registry.contains_key(canonical),
                "historical id {historical} must map to a registered canonical id, got {canonical}"
            );
            assert!(
                !registry.contains_key(historical),
                "historical id {historical} must not remain registered"
            );
            assert_eq!(canonical_transform_id(canonical), *canonical);
        }
    }

    #[test]
    fn canonicalizes_every_previous_builtin_id() {
        for (historical, canonical) in [
            ("append_empty_user_message", "prompt_append_empty_user"),
            ("auto_cache_openai_prompt", "cache_openai_prompt"),
            ("compress_user_message_images", "image_compress_input"),
            ("force_stream", "stream_force"),
            ("set_field", "field_set"),
            ("strip_reasoning", "reasoning_strip_output"),
            ("system_to_developer_role", "role_system_to_developer"),
            ("think_xml_to_reasoning", "reasoning_from_think_xml"),
        ] {
            assert_eq!(canonical_transform_id(historical), canonical);
        }
    }

    #[test]
    fn canonicalizes_legacy_transform_aliases() {
        let mut rule = TransformRuleConfig {
            transform: "remove_anthropic_billing_header".to_string(),
            enabled: true,
            models: None,
            phase: super::Phase::Request,
            config: serde_json::json!({}),
        };

        assert!(canonicalize_transform_rule(&mut rule));
        assert_eq!(rule.transform, "prompt_strip_anthropic_billing_header");
    }

    /// Regression test: rewriting must not stop at the first stale rule
    /// (TF-16 requires canonicalizing every persisted ID).
    #[test]
    fn canonicalizes_every_rule_in_a_chain_not_just_the_first() {
        let rule = |transform: &str| TransformRuleConfig {
            transform: transform.to_string(),
            enabled: true,
            models: None,
            phase: super::Phase::Request,
            config: serde_json::json!({}),
        };
        let mut rules = vec![
            rule("strip_reasoning"),
            rule("auto_cache_system"),
            rule("prompt_inject_system"),
            rule("set_field"),
        ];

        assert!(canonicalize_transform_rules(&mut rules));
        assert_eq!(
            rules
                .iter()
                .map(|r| r.transform.as_str())
                .collect::<Vec<_>>(),
            vec![
                "reasoning_strip_output",
                "cache_anthropic_system",
                "prompt_inject_system",
                "field_set",
            ]
        );
        // Idempotence: a second pass reports no change.
        assert!(!canonicalize_transform_rules(&mut rules));
    }
}

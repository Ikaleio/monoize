use crate::urp::{Message, Part, Role, UrpRequest, UrpResponse, UrpStreamEvent};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub mod append_empty_user_message;
pub mod force_stream;
pub mod inject_system_prompt;
pub mod merge_consecutive_roles;
pub mod override_max_tokens;
pub mod reasoning_effort_to_budget;
pub mod reasoning_effort_to_model_suffix;
pub mod reasoning_to_think_xml;
pub mod remove_field;
pub mod set_field;
pub mod strip_reasoning;
pub mod system_to_developer_role;
pub mod think_xml_to_reasoning;
pub mod auto_cache_system;
pub mod auto_cache_tool_use;
pub mod auto_cache_user_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Request,
    Response,
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

pub enum UrpData<'a> {
    Request(&'a mut UrpRequest),
    Response(&'a mut UrpResponse),
    Stream(&'a mut UrpStreamEvent),
}

impl<'a> UrpData<'a> {
    pub fn reborrow(&mut self) -> UrpData<'_> {
        match self {
            Self::Request(v) => UrpData::Request(*v),
            Self::Response(v) => UrpData::Response(*v),
            Self::Stream(v) => UrpData::Stream(*v),
        }
    }
}

pub trait TransformConfig: Send + Sync + 'static {
    fn as_any(&self) -> &dyn Any;
}

pub trait TransformState: Send + Sync {
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub struct NoState;

impl TransformState for NoState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub trait Transform: Send + Sync + 'static {
    fn type_id(&self) -> &'static str;
    fn supported_phases(&self) -> &'static [Phase];
    fn config_schema(&self) -> Value;
    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError>;
    fn init_state(&self) -> Box<dyn TransformState>;
    fn apply(
        &self,
        data: UrpData<'_>,
        phase: Phase,
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

pub struct TransformEntry {
    pub factory: fn() -> Box<dyn Transform>,
}

inventory::collect!(TransformEntry);

pub type TransformRegistry = HashMap<&'static str, Arc<dyn Transform>>;

pub fn registry() -> TransformRegistry {
    let mut map = HashMap::new();
    for entry in inventory::iter::<TransformEntry> {
        let transform = (entry.factory)();
        let type_id = Transform::type_id(&*transform);
        map.insert(type_id, Arc::<dyn Transform>::from(transform));
    }
    map
}

pub fn build_states_for_rules(
    rules: &[TransformRuleConfig],
    registry: &TransformRegistry,
) -> Result<Vec<Box<dyn TransformState>>, TransformError> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if let Some(transform) = registry.get(rule.transform.as_str()) {
            out.push(transform.init_state());
        } else {
            return Err(TransformError::NotFound(rule.transform.clone()));
        }
    }
    Ok(out)
}

pub fn apply_transforms(
    mut data: UrpData<'_>,
    rules: &[TransformRuleConfig],
    states: &mut [Box<dyn TransformState>],
    current_model: &str,
    phase: Phase,
    registry: &TransformRegistry,
) -> Result<(), TransformError> {
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
        let transform = registry
            .get(rule.transform.as_str())
            .ok_or_else(|| TransformError::NotFound(rule.transform.clone()))?;
        let config = transform.parse_config(rule.config.clone())?;
        transform.apply(data.reborrow(), phase, config.as_ref(), states[i].as_mut())?;
    }
    Ok(())
}

pub fn model_glob_match(pattern: &str, model: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    Regex::new(&regex)
        .map(|re| re.is_match(model))
        .unwrap_or(false)
}

pub fn text_part(content: impl Into<String>) -> Part {
    Part::Text {
        content: content.into(),
        extra_body: HashMap::new(),
    }
}

pub fn move_system_to_developer(messages: &mut [Message]) {
    for message in messages {
        if message.role == Role::System {
            message.role = Role::Developer;
        }
    }
}

pub fn merge_same_role_messages(messages: &[Message]) -> Vec<Message> {
    let mut merged: Vec<Message> = Vec::new();
    for message in messages {
        if let Some(last) = merged.last_mut() {
            if last.role == message.role {
                last.parts.extend(message.parts.clone());
                for (k, v) in &message.extra_body {
                    if !last.extra_body.contains_key(k) {
                        last.extra_body.insert(k.clone(), v.clone());
                    }
                }
                continue;
            }
        }
        merged.push(message.clone());
    }
    merged
}

pub fn strip_reasoning_parts(parts: &[Part]) -> Vec<Part> {
    parts
        .iter()
        .filter(|part| {
            !matches!(
                part,
                Part::Reasoning { .. } | Part::ReasoningEncrypted { .. }
            )
        })
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
    let mut cursor = extra.get_mut(&first).expect("extra key exists");
    for key in keys.iter().skip(1).take(keys.len().saturating_sub(2)) {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        let obj = cursor.as_object_mut().expect("object value");
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

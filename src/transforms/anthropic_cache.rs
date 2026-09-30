//! Shared `ttl` config and marker ownership for the Anthropic `cache_*` transforms
//! (`spec/auto-cache-transforms.spec.md` DEF-11 through DEF-15).

use crate::transforms::{TransformConfig, TransformError};
use crate::urp::AnthropicCacheTarget;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Default, Deserialize)]
enum CacheTtl {
    #[default]
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct CacheConfig {
    ttl: CacheTtl,
}

impl TransformConfig for CacheConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CacheConfig {
    pub(super) fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "ttl": {
                    "type": "string",
                    "enum": ["5m", "1h"],
                    "default": "5m"
                }
            },
            "additionalProperties": false
        })
    }

    pub(super) fn parse(raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Self = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        Ok(Box::new(cfg))
    }

    pub(super) fn from_dyn(config: &dyn TransformConfig) -> Result<&Self, TransformError> {
        config
            .as_any()
            .downcast_ref::<Self>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))
    }

    /// Applies this rule's marker to one target. A marker inserted by an earlier Monoize rule is
    /// retuned, so later scopes (API key after Provider) win; a client marker is left untouched.
    /// `slot_free` gates only the creation of a new breakpoint.
    pub(super) fn apply_marker(
        &self,
        extra_body: &mut HashMap<String, Value>,
        owned: &mut HashSet<AnthropicCacheTarget>,
        target: AnthropicCacheTarget,
        slot_free: bool,
    ) {
        match extra_body.get_mut("cache_control") {
            Some(marker) if owned.contains(&target) => *marker = self.cache_control(),
            Some(_) => {}
            None if slot_free => {
                extra_body.insert("cache_control".to_string(), self.cache_control());
                owned.insert(target);
            }
            None => {}
        }
    }

    /// `5m` omits `ttl` because Anthropic defaults to a 5-minute TTL, which keeps the wire
    /// format of older rules.
    fn cache_control(&self) -> Value {
        match self.ttl {
            CacheTtl::FiveMinutes => json!({"type": "ephemeral"}),
            CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
        }
    }
}

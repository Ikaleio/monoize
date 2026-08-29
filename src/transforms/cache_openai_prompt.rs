use crate::config::ProviderType;
use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{Node, OrdinaryRole, UrpRequest};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::any::Any;
use xxhash_rust::xxh3::Xxh3;

const DEFAULT_KEY_PREFIX: &str = "mzpc";
const DEFAULT_RETENTION: &str = "24h";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KeyMode {
    Prefix,
    Identity,
}

fn default_key_mode() -> KeyMode {
    KeyMode::Prefix
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    retention: String,
    key_prefix: String,
    key_mode: KeyMode,
    include_user_in_key: bool,
    include_full_input_in_key: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            retention: DEFAULT_RETENTION.to_string(),
            key_prefix: DEFAULT_KEY_PREFIX.to_string(),
            key_mode: default_key_mode(),
            include_user_in_key: false,
            include_full_input_in_key: false,
        }
    }
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct CacheOpenAiPromptTransform;

#[async_trait]
impl Transform for CacheOpenAiPromptTransform {
    fn type_id(&self) -> &'static str {
        "cache_openai_prompt"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Auto-cache: OpenAI prompt key"),
            ("zh", "自动缓存：OpenAI prompt 缓存键"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Derives a deterministic prompt_cache_key and retention for OpenAI Responses/Chat upstreams so repeated prompts hit the provider prompt cache.",
            ),
            (
                "zh",
                "为 OpenAI Responses/Chat 上游生成确定性的 prompt_cache_key 与保留策略，使重复请求命中官方提示词缓存。",
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
            "properties": {
                "retention": {
                    "type": "string",
                    "enum": ["24h", "in_memory"],
                    "default": DEFAULT_RETENTION
                },
                "key_prefix": {
                    "type": "string",
                    "default": DEFAULT_KEY_PREFIX
                },
                "key_mode": {
                    "type": "string",
                    "enum": ["prefix", "identity"],
                    "default": "prefix"
                },
                "include_user_in_key": {
                    "type": "boolean",
                    "default": false
                },
                "include_full_input_in_key": {
                    "type": "boolean",
                    "default": false
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let cfg: Config = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        if cfg.retention != "24h" && cfg.retention != "in_memory" {
            return Err(TransformError::InvalidConfig(
                "retention must be '24h' or 'in_memory'".to_string(),
            ));
        }
        if cfg.key_prefix.is_empty() {
            return Err(TransformError::InvalidConfig(
                "key_prefix must not be empty".to_string(),
            ));
        }
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
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let UrpData::Request(req) = data else {
            return Ok(());
        };
        if !matches!(
            context.upstream_provider_type,
            Some(ProviderType::Responses | ProviderType::ChatCompletion)
        ) {
            return Ok(());
        }

        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::Apply("invalid config type".to_string()))?;

        if !req.extra_body.contains_key("prompt_cache_key") {
            let key = build_prompt_cache_key(req, cfg)?;
            req.extra_body
                .insert("prompt_cache_key".to_string(), Value::String(key));
        }

        req.extra_body
            .entry("prompt_cache_retention".to_string())
            .or_insert_with(|| Value::String(cfg.retention.clone()));

        Ok(())
    }
}

fn build_prompt_cache_key(req: &UrpRequest, cfg: &Config) -> Result<String, TransformError> {
    let material = build_key_material(req, cfg)?;
    let serialized = serde_json::to_vec(&material)
        .map_err(|e| TransformError::Apply(format!("serialize cache key material failed: {e}")))?;
    let mut hasher = Xxh3::new();
    hasher.update(&serialized);
    let digest = format!("{:032x}", hasher.digest128());
    Ok(format!("{}_{}", cfg.key_prefix, digest))
}

fn build_key_material(req: &UrpRequest, cfg: &Config) -> Result<Value, TransformError> {
    let mut material = Map::new();

    match cfg.key_mode {
        KeyMode::Prefix => build_prefix_key_material(req, cfg, &mut material)?,
        KeyMode::Identity => build_identity_key_material(req, &mut material),
    }

    Ok(Value::Object(material))
}

fn build_prefix_key_material(
    req: &UrpRequest,
    cfg: &Config,
    material: &mut Map<String, Value>,
) -> Result<(), TransformError> {
    material.insert("model".to_string(), Value::String(req.model.clone()));

    if cfg.include_full_input_in_key {
        material.insert("input".to_string(), to_value(&req.input)?);
    } else {
        let prefix_nodes: Vec<Node> = req
            .input
            .iter()
            .take_while(|node| {
                matches!(
                    node.role(),
                    Some(OrdinaryRole::System | OrdinaryRole::Developer)
                )
            })
            .cloned()
            .collect();
        material.insert("prefix_nodes".to_string(), to_value(prefix_nodes)?);
    }

    if let Some(tools) = &req.tools {
        material.insert("tools".to_string(), to_value(tools)?);
    }
    if let Some(response_format) = &req.response_format {
        material.insert("response_format".to_string(), to_value(response_format)?);
    }
    if cfg.include_user_in_key {
        if let Some(user) = &req.user {
            material.insert("user".to_string(), Value::String(user.clone()));
        } else if let Some(username) = req
            .extra_body
            .get("__monoize_username")
            .and_then(Value::as_str)
        {
            material.insert("user".to_string(), Value::String(username.to_string()));
        }
    }

    Ok(())
}

fn build_identity_key_material(req: &UrpRequest, material: &mut Map<String, Value>) {
    material.insert(
        "username".to_string(),
        req.extra_body
            .get("__monoize_username")
            .and_then(Value::as_str)
            .map_or(Value::Null, |v| Value::String(v.to_string())),
    );
    material.insert(
        "api_key_id".to_string(),
        req.extra_body
            .get("__monoize_api_key_id")
            .and_then(Value::as_str)
            .map_or(Value::Null, |v| Value::String(v.to_string())),
    );
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, TransformError> {
    serde_json::to_value(value)
        .map_err(|e| TransformError::Apply(format!("serialize cache key material failed: {e}")))
}

inventory::submit!(TransformEntry {
    factory: || Box::new(CacheOpenAiPromptTransform),
});

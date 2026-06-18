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

pub struct AutoCacheOpenAiPromptTransform;

#[async_trait]
impl Transform for AutoCacheOpenAiPromptTransform {
    fn type_id(&self) -> &'static str {
        "auto_cache_openai_prompt"
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
    factory: || Box::new(AutoCacheOpenAiPromptTransform),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::UrpData;
    use crate::urp::{FunctionDefinition, ResponseFormat, ToolDefinition};
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn context(provider_type: Option<ProviderType>) -> TransformRuntimeContext {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("cache");
        TransformRuntimeContext {
            image_transform_cache: std::sync::Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: provider_type,
        }
    }

    fn request_with_user_message(user_text: &str) -> UrpRequest {
        UrpRequest {
            model: "gpt-5.5".to_string(),
            input: vec![
                Node::Text {
                    id: None,
                    role: OrdinaryRole::System,
                    content: "You are a coding assistant.".to_string(),
                    phase: None,
                    extra_body: HashMap::new(),
                },
                Node::Text {
                    id: None,
                    role: OrdinaryRole::User,
                    content: user_text.to_string(),
                    phase: None,
                    extra_body: HashMap::new(),
                },
            ],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: Some(vec![ToolDefinition {
                tool_type: "function".to_string(),
                name: None,
                description: None,
                function: Some(FunctionDefinition {
                    name: "lookup".to_string(),
                    description: Some("Lookup data".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "q": { "type": "string" }
                        }
                    })),
                    strict: Some(true),
                    extra_body: HashMap::new(),
                }),
                custom: None,
                extra_body: HashMap::new(),
            }]),
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: Some(ResponseFormat::Text),
            user: None,
            extra_body: HashMap::new(),
        }
    }

    #[test]
    fn prompt_cache_key_uses_128_bit_hex_digest() {
        let cfg = Config::default();
        let req = request_with_user_message("question");
        let key = build_prompt_cache_key(&req, &cfg).expect("cache key");
        let Some(suffix) = key.strip_prefix("mzpc_") else {
            panic!("expected default prefix");
        };

        assert_eq!(suffix.len(), 32);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!suffix.chars().any(|c| c.is_ascii_uppercase()));
    }

    #[test]
    fn identity_mode_uses_username_and_api_key_id_only() {
        let cfg = Config {
            key_mode: KeyMode::Identity,
            ..Config::default()
        };
        let mut first = request_with_user_message("first question");
        let mut second = request_with_user_message("different question");
        for req in [&mut first, &mut second] {
            req.extra_body
                .insert("__monoize_username".to_string(), json!("alice"));
            req.extra_body
                .insert("__monoize_api_key_id".to_string(), json!("key-a"));
        }
        first.model = "gpt-5.4-mini".to_string();
        second.model = "gpt-5.5".to_string();

        let first_key = build_prompt_cache_key(&first, &cfg).expect("first key");
        let second_key = build_prompt_cache_key(&second, &cfg).expect("second key");

        second
            .extra_body
            .insert("__monoize_api_key_id".to_string(), json!("key-b"));
        let third_key = build_prompt_cache_key(&second, &cfg).expect("third key");

        assert_eq!(first_key, second_key);
        assert_ne!(first_key, third_key);
    }

    #[tokio::test]
    async fn inserts_stable_openai_prompt_cache_fields_for_responses_provider() {
        let transform = AutoCacheOpenAiPromptTransform;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut first = request_with_user_message("first question");
        let mut second = request_with_user_message("different question");

        for req in [&mut first, &mut second] {
            transform
                .apply(
                    UrpData::Request(req),
                    Phase::Request,
                    &context(Some(ProviderType::Responses)).await,
                    cfg.as_ref(),
                    state.as_mut(),
                )
                .await
                .expect("apply");
        }

        assert_eq!(
            first.extra_body.get("prompt_cache_key"),
            second.extra_body.get("prompt_cache_key")
        );
        assert_eq!(
            first.extra_body.get("prompt_cache_retention"),
            Some(&Value::String("24h".to_string()))
        );
    }

    #[tokio::test]
    async fn can_include_full_input_when_configured() {
        let transform = AutoCacheOpenAiPromptTransform;
        let cfg = transform
            .parse_config(json!({ "include_full_input_in_key": true }))
            .expect("config");
        let mut state = transform.init_state();
        let mut first = request_with_user_message("first question");
        let mut second = request_with_user_message("different question");

        for req in [&mut first, &mut second] {
            transform
                .apply(
                    UrpData::Request(req),
                    Phase::Request,
                    &context(Some(ProviderType::ChatCompletion)).await,
                    cfg.as_ref(),
                    state.as_mut(),
                )
                .await
                .expect("apply");
        }

        assert_ne!(
            first.extra_body.get("prompt_cache_key"),
            second.extra_body.get("prompt_cache_key")
        );
    }

    #[tokio::test]
    async fn does_not_run_for_non_openai_provider() {
        let transform = AutoCacheOpenAiPromptTransform;
        let cfg = transform.parse_config(json!({})).expect("config");
        let mut state = transform.init_state();
        let mut req = request_with_user_message("question");

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context(Some(ProviderType::Messages)).await,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        assert!(req.extra_body.get("prompt_cache_key").is_none());
        assert!(req.extra_body.get("prompt_cache_retention").is_none());
    }

    #[tokio::test]
    async fn preserves_explicit_openai_cache_fields() {
        let transform = AutoCacheOpenAiPromptTransform;
        let cfg = transform
            .parse_config(json!({ "retention": "in_memory" }))
            .expect("config");
        let mut state = transform.init_state();
        let mut req = request_with_user_message("question");
        req.extra_body.insert(
            "prompt_cache_key".to_string(),
            Value::String("client-key".to_string()),
        );
        req.extra_body.insert(
            "prompt_cache_retention".to_string(),
            Value::String("24h".to_string()),
        );

        transform
            .apply(
                UrpData::Request(&mut req),
                Phase::Request,
                &context(Some(ProviderType::Responses)).await,
                cfg.as_ref(),
                state.as_mut(),
            )
            .await
            .expect("apply");

        assert_eq!(
            req.extra_body.get("prompt_cache_key"),
            Some(&Value::String("client-key".to_string()))
        );
        assert_eq!(
            req.extra_body.get("prompt_cache_retention"),
            Some(&Value::String("24h".to_string()))
        );
    }
}

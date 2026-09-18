use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{
    Node, NodeHeader, ToolChoice, ToolDefinition, UrpRequest, UrpResponse, UrpStreamEvent,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::any::Any;
use std::collections::{HashMap, HashSet};

const DEFAULT_ALIASES: &[(&str, &str)] = &[("view_image", "client_view_image")];

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    aliases: Option<Map<String, Value>>,
}

struct Config {
    forward: HashMap<String, String>,
    reverse: HashMap<String, String>,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn default_alias_map() -> HashMap<String, String> {
    DEFAULT_ALIASES
        .iter()
        .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
        .collect()
}

fn parse_alias_map(
    raw: Option<Map<String, Value>>,
) -> Result<HashMap<String, String>, TransformError> {
    let Some(obj) = raw else {
        return Ok(default_alias_map());
    };
    let mut forward = HashMap::new();
    let mut seen_aliases = HashSet::new();
    for (original, value) in obj {
        let Some(alias) = value.as_str() else {
            return Err(TransformError::InvalidConfig(format!(
                "aliases.{original} must be a string"
            )));
        };
        if original.is_empty() || alias.is_empty() {
            return Err(TransformError::InvalidConfig(
                "alias keys and values must be non-empty strings".to_string(),
            ));
        }
        if original == alias {
            return Err(TransformError::InvalidConfig(format!(
                "alias for {original} must differ from the original name"
            )));
        }
        if !seen_aliases.insert(alias.to_string()) {
            return Err(TransformError::InvalidConfig(format!(
                "alias {alias} is mapped from more than one original name"
            )));
        }
        if forward
            .insert(original.clone(), alias.to_string())
            .is_some()
        {
            return Err(TransformError::InvalidConfig(format!(
                "duplicate alias key {original}"
            )));
        }
    }
    for original in forward.keys() {
        if seen_aliases.contains(original) {
            return Err(TransformError::InvalidConfig(format!(
                "alias {original} collides with an original tool name"
            )));
        }
    }
    Ok(forward)
}

fn reverse_map(forward: &HashMap<String, String>) -> HashMap<String, String> {
    forward
        .iter()
        .map(|(original, alias)| (alias.clone(), original.clone()))
        .collect()
}

fn rename_string(name: &mut String, map: &HashMap<String, String>) {
    if let Some(renamed) = map.get(name) {
        *name = renamed.clone();
    }
}

fn rename_json_tool_names(value: &mut Value, map: &HashMap<String, String>) {
    match value {
        Value::Object(obj) => {
            if let Some(Value::String(name)) = obj.get_mut("name") {
                rename_string(name, map);
            }
            for nested in obj.values_mut() {
                rename_json_tool_names(nested, map);
            }
        }
        Value::Array(items) => {
            for item in items {
                rename_json_tool_names(item, map);
            }
        }
        _ => {}
    }
}

fn is_renamable_tool_type(tool_type: &str) -> bool {
    matches!(tool_type, "function" | "custom")
}

fn rename_tool_definition(tool: &mut ToolDefinition, map: &HashMap<String, String>) {
    if !is_renamable_tool_type(&tool.tool_type) {
        return;
    }
    if let Some(name) = tool.name.as_mut() {
        rename_string(name, map);
    }
    if let Some(function) = tool.function.as_mut() {
        rename_string(&mut function.name, map);
    }
    if let Some(custom) = tool.custom.as_mut() {
        rename_string(&mut custom.name, map);
    }
}

fn rename_node(node: &mut Node, map: &HashMap<String, String>) {
    if let Node::ToolCall { name, .. } = node {
        rename_string(name, map);
    }
}

fn rename_header(header: &mut NodeHeader, map: &HashMap<String, String>) {
    if let NodeHeader::ToolCall { name, .. } = header {
        rename_string(name, map);
    }
}

fn apply_request(req: &mut UrpRequest, map: &HashMap<String, String>) {
    if let Some(tools) = req.tools.as_mut() {
        for tool in tools {
            rename_tool_definition(tool, map);
        }
    }
    for node in &mut req.input {
        rename_node(node, map);
    }
    if let Some(ToolChoice::Specific(value)) = req.tool_choice.as_mut() {
        rename_json_tool_names(value, map);
    }
}

fn apply_response(resp: &mut UrpResponse, map: &HashMap<String, String>) {
    for node in &mut resp.output {
        rename_node(node, map);
    }
}

fn apply_stream(event: &mut UrpStreamEvent, map: &HashMap<String, String>) {
    match event {
        UrpStreamEvent::NodeStart { header, .. } => rename_header(header, map),
        UrpStreamEvent::NodeDone { node, .. } => rename_node(node, map),
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                rename_node(node, map);
            }
        }
        _ => {}
    }
}

pub struct FieldAliasReservedToolNamesTransform;

#[async_trait]
impl Transform for FieldAliasReservedToolNamesTransform {
    fn type_id(&self) -> &'static str {
        "field_alias_reserved_tool_names"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Field: alias reserved tool names"),
            ("zh", "字段：为保留工具名创建别名"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Renames client function tools that collide with upstream reserved server-side names, then restores the original names on the response.",
            ),
            (
                "zh",
                "将与上游服务端保留名冲突的客户端函数工具改名，并在响应中恢复原名。",
            ),
        ]
    }

    fn supported_phases(&self) -> &'static [Phase] {
        &[Phase::Request, Phase::Response]
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
                "aliases": {
                    "type": "object",
                    "additionalProperties": { "type": "string", "minLength": 1 },
                    "description": "Map of original function names to upstream-safe aliases. Omit to use the default view_image -> client_view_image mapping. An empty object disables renaming."
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let raw_cfg: RawConfig = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        let forward = parse_alias_map(raw_cfg.aliases)?;
        let reverse = reverse_map(&forward);
        Ok(Box::new(Config { forward, reverse }))
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(NoState)
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        phase: Phase,
        _context: &TransformRuntimeContext,
        config: &dyn TransformConfig,
        _state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let cfg = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| TransformError::InvalidConfig("internal config type mismatch".into()))?;
        let map = match phase {
            Phase::Request => &cfg.forward,
            Phase::Response => &cfg.reverse,
        };
        if map.is_empty() {
            return Ok(());
        }
        match data {
            UrpData::Request(req) => apply_request(req, map),
            UrpData::Response(resp) => apply_response(resp, map),
            UrpData::Stream(event) => apply_stream(event, map),
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(FieldAliasReservedToolNamesTransform),
});

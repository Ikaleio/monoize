use crate::transforms::{
    NoState, Phase, Transform, TransformConfig, TransformEntry, TransformError,
    TransformRuntimeContext, TransformScope, TransformState, UrpData,
};
use crate::urp::{
    FunctionDefinition, Node, NodeHeader, ToolCallType, ToolChoice, ToolDefinition, UrpRequest,
    UrpResponse, UrpStreamEvent,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashSet;

const DEFAULT_NAMES: &[&str] = &["apply_patch"];
const INPUT_KEYS: &[&str] = &["input", "patch", "command", "content"];

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    names: Option<Vec<String>>,
}

struct Config {
    names: HashSet<String>,
    convert_all: bool,
}

impl TransformConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn parse_names(raw: Option<Vec<String>>) -> Result<Config, TransformError> {
    let names = match raw {
        None => DEFAULT_NAMES
            .iter()
            .map(|n| (*n).to_string())
            .collect::<Vec<_>>(),
        Some(list) => list,
    };
    let mut set = HashSet::new();
    let mut convert_all = false;
    for name in names {
        if name.is_empty() {
            return Err(TransformError::InvalidConfig(
                "names entries must be non-empty strings".to_string(),
            ));
        }
        if name == "*" {
            convert_all = true;
            continue;
        }
        set.insert(name);
    }
    Ok(Config {
        names: set,
        convert_all,
    })
}

fn custom_name(tool: &ToolDefinition) -> Option<&str> {
    if tool.tool_type != "custom" {
        return None;
    }
    tool.custom
        .as_ref()
        .map(|c| c.name.as_str())
        .or(tool.name.as_deref())
}

fn should_convert(cfg: &Config, name: &str) -> bool {
    cfg.convert_all || cfg.names.contains(name)
}

fn wrap_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
        if map.contains_key("input") {
            return raw.to_string();
        }
    }
    json!({ "input": raw }).to_string()
}

fn unwrap_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Value::Object(map) = value {
            for key in INPUT_KEYS {
                if let Some(Value::String(s)) = map.get(*key) {
                    if *key == "input" || s.contains("Begin Patch") {
                        return s.clone();
                    }
                }
            }
        } else if let Value::String(s) = value {
            return s;
        }
    }
    raw.to_string()
}

fn strip_xml_invoke(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("<invoke") else {
        return raw.to_string();
    };
    let Some(gt) = rest.find('>') else {
        return raw.to_string();
    };
    let inner = &rest[gt + 1..];
    if let Some(end) = inner.rfind("</invoke>") {
        inner[..end].trim().to_string()
    } else {
        inner.trim().to_string()
    }
}

fn normalize_apply_patch(raw: &str) -> String {
    let stripped = strip_xml_invoke(raw);
    let mut out = String::new();
    let mut in_add_file = false;
    for line in stripped.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("*** End of File ***") || trimmed == "*** End of File" {
            continue;
        }
        if trimmed.starts_with("*** Begin Patch") {
            out.push_str("*** Begin Patch\n");
            in_add_file = false;
            continue;
        }
        if trimmed.starts_with("*** End Patch") {
            out.push_str("*** End Patch\n");
            in_add_file = false;
            continue;
        }
        if trimmed.starts_with("*** Add File") {
            out.push_str(line);
            out.push('\n');
            in_add_file = true;
            continue;
        }
        if trimmed.starts_with("***") {
            out.push_str(line);
            out.push('\n');
            in_add_file = false;
            continue;
        }
        if in_add_file && !line.starts_with('+') {
            out.push('+');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn function_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "input": {
                "type": "string",
                "description": "The entire apply_patch document. First line must be exactly `*** Begin Patch`. Last line must be exactly `*** End Patch`. After `*** Add File: path`, every content line must start with `+`."
            }
        },
        "required": ["input"]
    })
}

fn convert_tool_to_function(tool: &mut ToolDefinition, cfg: &Config) {
    let Some(name) = custom_name(tool).map(str::to_string) else {
        return;
    };
    if !should_convert(cfg, &name) {
        return;
    }
    let custom = tool.custom.take();
    let description = custom.and_then(|c| c.description).or_else(|| {
        Some(
            "Use the apply_patch tool to edit files. Pass the full patch text in `input`. Do not wrap the patch in extra JSON keys other than `input`. After `*** Add File: path`, prefix every content line with `+`.".to_string(),
        )
    });
    tool.tool_type = "function".to_string();
    tool.name = Some(name.clone());
    tool.function = Some(FunctionDefinition {
        name,
        description,
        parameters: Some(function_parameters()),
        strict: None,
        extra_body: Default::default(),
    });
}

fn convert_node_request(node: &mut Node, cfg: &Config) {
    let Node::ToolCall {
        tool_type,
        name,
        arguments,
        ..
    } = node
    else {
        return;
    };
    if *tool_type != ToolCallType::Custom || !should_convert(cfg, name) {
        return;
    }
    *tool_type = ToolCallType::Function;
    *arguments = wrap_input(arguments);
}

fn convert_node_response(node: &mut Node, cfg: &Config) {
    let Node::ToolCall {
        tool_type,
        name,
        arguments,
        ..
    } = node
    else {
        return;
    };
    if !should_convert(cfg, name) {
        return;
    }
    if *tool_type == ToolCallType::Function {
        *tool_type = ToolCallType::Custom;
        *arguments = unwrap_input(arguments);
    }
    if name == "apply_patch" {
        *arguments = normalize_apply_patch(arguments);
    }
}

fn convert_header_response(header: &mut NodeHeader, cfg: &Config) {
    let NodeHeader::ToolCall {
        tool_type, name, ..
    } = header
    else {
        return;
    };
    if *tool_type != ToolCallType::Function || !should_convert(cfg, name) {
        return;
    }
    *tool_type = ToolCallType::Custom;
}

fn rewrite_tool_choice_request(value: &mut Value, cfg: &Config) {
    match value {
        Value::Object(obj) => {
            let type_is_custom = obj.get("type").and_then(Value::as_str) == Some("custom");
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string);
            if type_is_custom {
                if let Some(name) = name.as_deref() {
                    if should_convert(cfg, name) {
                        obj.insert("type".to_string(), Value::String("function".to_string()));
                    }
                }
            }
            for nested in obj.values_mut() {
                rewrite_tool_choice_request(nested, cfg);
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_tool_choice_request(item, cfg);
            }
        }
        _ => {}
    }
}

fn apply_request(req: &mut UrpRequest, cfg: &Config) {
    if let Some(tools) = req.tools.as_mut() {
        for tool in tools {
            convert_tool_to_function(tool, cfg);
        }
    }
    for node in &mut req.input {
        convert_node_request(node, cfg);
    }
    if let Some(ToolChoice::Specific(value)) = req.tool_choice.as_mut() {
        rewrite_tool_choice_request(value, cfg);
    }
}

fn apply_response(resp: &mut UrpResponse, cfg: &Config) {
    for node in &mut resp.output {
        convert_node_response(node, cfg);
    }
}

fn apply_stream(event: &mut UrpStreamEvent, cfg: &Config) {
    match event {
        UrpStreamEvent::NodeStart { header, .. } => convert_header_response(header, cfg),
        UrpStreamEvent::NodeDelta { delta, .. } => {
            if let crate::urp::NodeDelta::ToolCallArguments { arguments } = delta {
                let unwrapped = normalize_apply_patch(&unwrap_input(arguments));
                if unwrapped != *arguments {
                    *arguments = unwrapped;
                }
            }
        }
        UrpStreamEvent::NodeDone { node, .. } => convert_node_response(node, cfg),
        UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                convert_node_response(node, cfg);
            }
        }
        _ => {}
    }
}

pub struct FieldCustomToolsToFunctionTransform;

#[async_trait]
impl Transform for FieldCustomToolsToFunctionTransform {
    fn type_id(&self) -> &'static str {
        "field_custom_tools_to_function"
    }

    fn display_name(&self) -> crate::transforms::LocalizedText {
        &[
            ("en", "Field: custom tools to function"),
            ("zh", "字段：自定义工具转函数"),
        ]
    }

    fn display_description(&self) -> crate::transforms::LocalizedText {
        &[
            (
                "en",
                "Converts Codex freeform/custom tools such as apply_patch into JSON function tools for Grok/CPA-style upstreams, then restores custom tool calls on the response.",
            ),
            (
                "zh",
                "将 Codex 的 apply_patch 等 custom/freeform 工具转成 JSON function，供 Grok/CPA 上游使用，并在响应中恢复为 custom 工具调用。",
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
                "names": {
                    "type": "array",
                    "items": { "type": "string", "minLength": 1 },
                    "default": ["apply_patch"],
                    "description": "Custom tool names to convert. Omit for apply_patch only. Include \"*\" to convert every custom tool."
                }
            },
            "additionalProperties": false
        })
    }

    fn parse_config(&self, raw: Value) -> Result<Box<dyn TransformConfig>, TransformError> {
        let raw_cfg: RawConfig = serde_json::from_value(raw)
            .map_err(|e| TransformError::InvalidConfig(e.to_string()))?;
        Ok(Box::new(parse_names(raw_cfg.names)?))
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
        if !cfg.convert_all && cfg.names.is_empty() {
            return Ok(());
        }
        match (phase, data) {
            (Phase::Request, UrpData::Request(req)) => apply_request(req, cfg),
            (Phase::Response, UrpData::Response(resp)) => apply_response(resp, cfg),
            (Phase::Response, UrpData::Stream(event)) => apply_stream(event, cfg),
            _ => {}
        }
        Ok(())
    }
}

inventory::submit!(TransformEntry {
    factory: || Box::new(FieldCustomToolsToFunctionTransform),
});

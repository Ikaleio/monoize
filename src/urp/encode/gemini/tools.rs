//! Gemini `tools` and `toolConfig` encoding (PG4g, PG4h, PG4j, GEM-4).

use crate::urp::decode::gemini::native_schema_types;
use crate::urp::{
    FunctionDefinition, GEMINI_PARAMETERS_JSON_SCHEMA_EXTRA_KEY,
    GEMINI_RESPONSE_JSON_SCHEMA_EXTRA_KEY, ProviderProtocol, ToolChoice, ToolDefinition,
    UrpRequest, is_internal_key,
};
use serde_json::{Map, Value, json};

/// Gemini Tool member equivalent to a cross-protocol built-in tool type.
pub fn gemini_builtin_tool_key(tool_type: &str) -> Option<&'static str> {
    if tool_type == "web_search" || tool_type.starts_with("web_search_") {
        Some("googleSearch")
    } else if tool_type == "code_interpreter" || tool_type.starts_with("code_execution") {
        Some("codeExecution")
    } else if tool_type.starts_with("web_fetch") {
        Some("urlContext")
    } else {
        None
    }
}

/// True for a cross-protocol built-in tool the Gemini encoder can represent.
pub fn is_gemini_mappable_tool(tool_type: &str) -> bool {
    gemini_builtin_tool_key(tool_type).is_some() || tool_type == "image_generation"
}

/// PG4h: a non-function tool without a Gemini mapping or Gemini origin fails the request.
pub(super) fn validate_tools(req: &UrpRequest) -> Result<(), String> {
    for tool in req.tools.as_deref().unwrap_or_default() {
        if tool.tool_type != "function"
            && tool.origin_protocol != Some(ProviderProtocol::Gemini)
            && !is_gemini_mappable_tool(&tool.tool_type)
        {
            return Err(format!(
                "Gemini has no equivalent for tool type {}.",
                tool.tool_type
            ));
        }
    }
    Ok(())
}

/// The `tools` member: one Tool holding every function declaration, then each Gemini-native
/// tool with its typed config, then each mapped cross-family built-in once.
pub(super) fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut encoded = Vec::new();
    let declarations: Vec<Value> = tools
        .iter()
        .filter(|tool| tool.tool_type == "function")
        .filter_map(|tool| tool.function.as_ref())
        .map(encode_function_declaration)
        .collect();
    if !declarations.is_empty() {
        encoded.push(Value::Object(Map::from_iter([(
            "functionDeclarations".to_string(),
            Value::Array(declarations),
        )])));
    }
    let mut builtin = Vec::new();
    for tool in tools {
        if tool.origin_protocol == Some(ProviderProtocol::Gemini) {
            if let Some(config) = &tool.config {
                encoded.push(json!({tool.tool_type.clone():config}));
            }
        } else if let Some(key) = gemini_builtin_tool_key(&tool.tool_type)
            && !builtin.contains(&key)
        {
            builtin.push(key);
            encoded.push(json!({ key: {} }));
        }
    }
    encoded
}

fn encode_function_declaration(function: &FunctionDefinition) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), json!(function.name));
    // FunctionDeclaration.description is a required field.
    obj.insert(
        "description".into(),
        json!(function.description.as_deref().unwrap_or_default()),
    );
    // The native placement marker selects `Schema` members over the JSON Schema members.
    let native =
        |marker: &str| function.extra_body.get(marker).and_then(Value::as_bool) == Some(false);
    if let Some(parameters) = &function.parameters {
        if native(GEMINI_PARAMETERS_JSON_SCHEMA_EXTRA_KEY) {
            obj.insert("parameters".into(), native_schema_types(parameters, true));
        } else {
            obj.insert("parametersJsonSchema".into(), parameters.clone());
        }
    }
    if let Some(schema) = &function.response_schema {
        if native(GEMINI_RESPONSE_JSON_SCHEMA_EXTRA_KEY) {
            obj.insert("response".into(), native_schema_types(schema, true));
        } else {
            obj.insert("responseJsonSchema".into(), schema.clone());
        }
    }
    // Typed members own their names: native extras never restore a deleted schema.
    for (key, value) in &function.extra_body {
        if !is_internal_key(key)
            && !obj.contains_key(key)
            && !matches!(
                key.as_str(),
                "name"
                    | "description"
                    | "parameters"
                    | "parametersJsonSchema"
                    | "response"
                    | "responseJsonSchema"
            )
        {
            obj.insert(key.clone(), value.clone());
        }
    }
    Value::Object(obj)
}

/// The `toolConfig` member: retained native members with the typed function-calling mode.
pub(super) fn encode_tool_config(req: &UrpRequest) -> Map<String, Value> {
    let mut config = req
        .extra_body
        .get("toolConfig")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut function_config = config
        .remove("functionCallingConfig")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    function_config.remove("mode");
    function_config.remove("allowedFunctionNames");
    // Function-calling modes constrain function declarations only; without declarations a
    // typed choice has nothing to select and is omitted.
    let has_declarations = req.tools.as_deref().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool.tool_type == "function" && tool.function.is_some())
    });
    if has_declarations && let Some(choice) = req.tool_choice.as_ref().and_then(encode_tool_choice)
    {
        function_config.extend(choice);
    }
    if !function_config.is_empty() {
        config.insert(
            "functionCallingConfig".into(),
            Value::Object(function_config),
        );
    }
    config
}

fn encode_tool_choice(choice: &ToolChoice) -> Option<Map<String, Value>> {
    let (mode, names) = match choice {
        ToolChoice::Mode(mode) => (
            match mode.as_str() {
                "none" => "NONE",
                "required" => "ANY",
                "validated" => "VALIDATED",
                _ => "AUTO",
            },
            Vec::new(),
        ),
        ToolChoice::Specific(value)
            if value.get("type").and_then(Value::as_str) == Some("allowed_tools") =>
        {
            // Responses uses a flat selector; Chat nests it under `allowed_tools`.
            let selector = value
                .get("allowed_tools")
                .filter(|nested| nested.is_object())
                .unwrap_or(value);
            let names: Vec<Value> = selector
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tool| {
                    tool.get("name").or_else(|| {
                        tool.get("function")
                            .and_then(|function| function.get("name"))
                    })
                })
                .filter(|name| name.is_string())
                .cloned()
                .collect();
            // allowedFunctionNames is valid only with ANY or VALIDATED; an `auto` choice
            // restricted to named functions is Gemini VALIDATED.
            let mode = match selector.get("mode").and_then(Value::as_str) {
                Some("required" | "any") => "ANY",
                Some("none") => "NONE",
                Some("validated") => "VALIDATED",
                _ if !names.is_empty() => "VALIDATED",
                _ => "AUTO",
            };
            (mode, if mode == "NONE" { Vec::new() } else { names })
        }
        ToolChoice::Specific(value) => {
            let name = value
                .get("name")
                .or_else(|| {
                    value
                        .get("function")
                        .and_then(|function| function.get("name"))
                })
                .and_then(Value::as_str)?;
            ("ANY", vec![json!(name)])
        }
    };
    let mut config = Map::new();
    config.insert("mode".into(), json!(mode));
    if !names.is_empty() {
        config.insert("allowedFunctionNames".into(), Value::Array(names));
    }
    Some(config)
}

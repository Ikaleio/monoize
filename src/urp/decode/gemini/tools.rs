//! Gemini `tools` and `toolConfig.functionCallingConfig` decoding (GEM-4, PG4g, PG4h).

use super::schema::native_schema_types;
use crate::urp::decode::split_extra;
use crate::urp::{
    FunctionDefinition, GEMINI_PARAMETERS_JSON_SCHEMA_EXTRA_KEY,
    GEMINI_RESPONSE_JSON_SCHEMA_EXTRA_KEY, ProviderProtocol, ToolChoice, ToolDefinition,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Function declarations become function tools; every other Tool member is a Gemini-native
/// built-in tool whose typed `config` owns the native value.
pub(super) fn decode_tools(tools: &[Value]) -> Vec<ToolDefinition> {
    let mut out = Vec::new();
    for tool in tools.iter().filter_map(Value::as_object) {
        out.extend(
            tool.get("functionDeclarations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .filter_map(decode_function_declaration),
        );
        for (kind, config) in tool {
            if kind == "functionDeclarations" {
                continue;
            }
            out.push(ToolDefinition {
                namespace: None,
                tools: None,
                origin_protocol: Some(ProviderProtocol::Gemini),
                config: Some(config.clone()),
                tool_type: kind.clone(),
                name: None,
                description: None,
                function: None,
                custom: None,
                extra_body: HashMap::new(),
                cache_breakpoint: None,
            });
        }
    }
    out
}

fn decode_function_declaration(declaration: &Map<String, Value>) -> Option<ToolDefinition> {
    let name = declaration.get("name").and_then(Value::as_str)?;
    let mut extra_body = split_extra(
        declaration,
        &[
            "name",
            "description",
            "parameters",
            "parametersJsonSchema",
            "response",
            "responseJsonSchema",
        ],
    );
    // The typed schema owns the value; the marker records whether it used JSON Schema placement.
    for (marker, json_key, native_key) in [
        (
            GEMINI_PARAMETERS_JSON_SCHEMA_EXTRA_KEY,
            "parametersJsonSchema",
            "parameters",
        ),
        (
            GEMINI_RESPONSE_JSON_SCHEMA_EXTRA_KEY,
            "responseJsonSchema",
            "response",
        ),
    ] {
        if declaration.contains_key(json_key) || declaration.contains_key(native_key) {
            extra_body.insert(marker.into(), json!(declaration.contains_key(json_key)));
        }
    }
    let schema = |json_key: &str, native_key: &str| {
        declaration.get(json_key).cloned().or_else(|| {
            declaration
                .get(native_key)
                .map(|schema| native_schema_types(schema, false))
        })
    };
    Some(ToolDefinition {
        namespace: None,
        tools: None,
        origin_protocol: None,
        config: None,
        tool_type: "function".into(),
        name: None,
        description: None,
        custom: None,
        function: Some(FunctionDefinition {
            name: name.into(),
            description: declaration
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            parameters: schema("parametersJsonSchema", "parameters"),
            response_schema: schema("responseJsonSchema", "response"),
            strict: None,
            extra_body,
        }),
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    })
}

/// Typed tool choice of a `functionCallingConfig`. `allowedFunctionNames` becomes an
/// `allowed_tools` selector with the decoded mode.
pub(super) fn decode_tool_choice(config: &Value) -> Option<ToolChoice> {
    let config = config.as_object()?;
    let mode = match config.get("mode").and_then(Value::as_str) {
        Some("NONE") => "none",
        Some("ANY") => "required",
        Some("VALIDATED") => "validated",
        _ => "auto",
    };
    if let Some(names) = config.get("allowedFunctionNames").and_then(Value::as_array) {
        let tools: Vec<Value> = names
            .iter()
            .filter_map(Value::as_str)
            .map(|name| json!({"type":"function","name":name}))
            .collect();
        return Some(ToolChoice::Specific(
            json!({"type":"allowed_tools", "mode":mode, "tools":tools}),
        ));
    }
    Some(ToolChoice::Mode(mode.into()))
}

/// Unknown `toolConfig` members, including unknown `functionCallingConfig` members, which stay
/// request extras under their native names.
pub(super) fn tool_config_extra(config: &Map<String, Value>) -> HashMap<String, Value> {
    let mut extra = split_extra(config, &["functionCallingConfig"]);
    if let Some(function) = config
        .get("functionCallingConfig")
        .and_then(Value::as_object)
    {
        let unknown = split_extra(function, &["mode", "allowedFunctionNames"]);
        if !unknown.is_empty() {
            extra.insert(
                "functionCallingConfig".into(),
                Value::Object(unknown.into_iter().collect()),
            );
        }
    }
    extra
}

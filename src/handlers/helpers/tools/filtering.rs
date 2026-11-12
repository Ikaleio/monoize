use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderNativeToolFamily {
    Responses,
    Messages,
}

const RESPONSES_NATIVE_TOOL_TYPES: &[&str] = &[
    "file_search",
    "code_interpreter",
    "web_search",
    "web_search_preview",
    "mcp",
    "namespace",
    "tool_search",
    "programmatic_tool_calling",
    "image_generation",
    "computer",
    "computer_use_preview",
    "local_shell",
    "shell",
    "apply_patch",
];

const MESSAGES_NATIVE_TOOL_PREFIXES: &[&str] = &[
    "computer_",
    "web_search_",
    "web_fetch_",
    "code_execution_",
    "tool_search_tool_",
    "bash_",
    "text_editor_",
    "memory_",
    "advisor_",
];

const MESSAGES_NATIVE_TOOL_TYPES: &[&str] = &[
    "mcp_toolset",
    "tool_search_tool_bm25",
    "tool_search_tool_regex",
];

fn provider_native_tool_family(tool_type: &str) -> Option<ProviderNativeToolFamily> {
    if RESPONSES_NATIVE_TOOL_TYPES.contains(&tool_type) {
        return Some(ProviderNativeToolFamily::Responses);
    }
    if MESSAGES_NATIVE_TOOL_TYPES.contains(&tool_type)
        || MESSAGES_NATIVE_TOOL_PREFIXES
            .iter()
            .any(|prefix| tool_type.starts_with(prefix))
        || has_versioned_messages_native_tool_suffix(tool_type)
    {
        return Some(ProviderNativeToolFamily::Messages);
    }
    None
}

fn has_versioned_messages_native_tool_suffix(tool_type: &str) -> bool {
    tool_type
        .rsplit_once('_')
        .map(|(_, suffix)| suffix.len() == 8 && suffix.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(false)
}

fn provider_supports_native_tool_family(
    provider_type: ProviderType,
    family: ProviderNativeToolFamily,
) -> bool {
    matches!(
        (provider_type, family),
        (ProviderType::Responses, ProviderNativeToolFamily::Responses)
            | (ProviderType::Messages, ProviderNativeToolFamily::Messages)
    )
}

fn provider_supports_tool_definition(
    tool: &urp::ToolDefinition,
    provider_type: ProviderType,
    downstream: DownstreamProtocol,
) -> bool {
    if tool.tool_type == "function" {
        return true;
    }

    if tool.tool_type == "custom" {
        return provider_supports_custom_tool(tool, provider_type);
    }

    if provider_type == ProviderType::Gemini
        && tool.origin_protocol != Some(urp::ProviderProtocol::Gemini)
        && urp::encode::gemini::is_gemini_mappable_tool(&tool.tool_type)
    {
        return true;
    }

    // PC3: the Chat encoder maps web-search built-ins per dialect or drops them.
    if provider_type == ProviderType::ChatCompletion
        && urp::encode::openai_chat::is_web_search_builtin(&tool.tool_type)
    {
        return true;
    }

    if let Some(origin) = tool.origin_protocol {
        return matches!(
            (origin, provider_type),
            (urp::ProviderProtocol::Gemini, ProviderType::Gemini)
                | (urp::ProviderProtocol::Responses, ProviderType::Responses)
                | (urp::ProviderProtocol::Messages, ProviderType::Messages)
                | (
                    urp::ProviderProtocol::ChatCompletion,
                    ProviderType::ChatCompletion
                )
        );
    }

    if let Some(family) = provider_native_tool_family(&tool.tool_type) {
        return provider_supports_native_tool_family(provider_type, family);
    }

    downstream.is_same_family(provider_type)
        && matches!(
            provider_type,
            ProviderType::Responses | ProviderType::Messages
        )
}

fn provider_supports_custom_tool(tool: &urp::ToolDefinition, provider_type: ProviderType) -> bool {
    match provider_type {
        ProviderType::ChatCompletion | ProviderType::Responses => tool.custom.is_some(),
        ProviderType::Messages => tool.custom.as_ref().is_some_and(|custom| {
            custom.extra_body.contains_key("input_schema")
                || tool.extra_body.contains_key("input_schema")
        }),
        _ => false,
    }
}

pub(super) fn selector_name<'a>(
    obj: &'a serde_json::Map<String, Value>,
    kind: &str,
) -> Option<&'a str> {
    obj.get(kind)
        .and_then(Value::as_object)
        .and_then(|nested| nested.get("name"))
        .and_then(Value::as_str)
        .or_else(|| obj.get("name").and_then(Value::as_str))
}

fn selector_matches_tool(
    selector: &serde_json::Map<String, Value>,
    tool: &urp::ToolDefinition,
) -> bool {
    if let Some(namespace) = selector.get("namespace").and_then(Value::as_str) {
        if tool.tool_type != "namespace" || tool.name.as_deref() != Some(namespace) {
            return false;
        }
        let mut leaf_selector = selector.clone();
        leaf_selector.remove("namespace");
        return tool.tools.as_ref().is_some_and(|tools| {
            tools
                .iter()
                .any(|leaf| selector_matches_tool(&leaf_selector, leaf))
        });
    }
    match selector.get("type").and_then(Value::as_str) {
        Some("function") => {
            let available = match tool.tool_type.as_str() {
                "function" => tool
                    .function
                    .as_ref()
                    .map(|function| function.name.as_str()),
                // A named selector also addresses a native tool (Messages server tools).
                "custom" | "namespace" => None,
                _ => tool.name.as_deref(),
            };
            selector_name(selector, "function")
                .zip(available)
                .is_some_and(|(selected, available)| selected == available)
        }
        Some("custom") => {
            tool.tool_type == "custom"
                && selector_name(selector, "custom")
                    .zip(tool.custom.as_ref().map(|custom| custom.name.as_str()))
                    .is_some_and(|(selected, available)| selected == available)
        }
        Some("mcp") => {
            tool.tool_type == "mcp"
                && selector
                    .get("server_label")
                    .and_then(Value::as_str)
                    .zip(
                        tool.config
                            .as_ref()
                            .and_then(|config| config.get("server_label"))
                            .or_else(|| tool.extra_body.get("server_label"))
                            .and_then(Value::as_str),
                    )
                    .is_some_and(|(selected, available)| selected == available)
        }
        Some("auto" | "required" | "any" | "none" | "allowed_tools") | None => false,
        Some(native_type) => tool.tool_type == native_type,
    }
}

fn allowed_tool_references_mut(choice: &mut urp::ToolChoice) -> Option<&mut Vec<Value>> {
    let urp::ToolChoice::Specific(Value::Object(obj)) = choice else {
        return None;
    };
    if obj.get("type").and_then(Value::as_str) != Some("allowed_tools") {
        return None;
    }
    if obj.contains_key("allowed_tools") {
        return obj
            .get_mut("allowed_tools")
            .and_then(Value::as_object_mut)
            .and_then(|allowed| allowed.get_mut("tools"))
            .and_then(Value::as_array_mut);
    }
    obj.get_mut("tools").and_then(Value::as_array_mut)
}

/// T1g.1: the Responses and Messages web search definitions are mutual native equivalents.
fn map_web_search_tool_definition(
    tool: &mut urp::ToolDefinition,
    provider_type: ProviderType,
) -> bool {
    let config = tool.config.as_ref().and_then(Value::as_object);
    let mut mapped = Map::new();
    if let Some(location) = config.and_then(|config| config.get("user_location")) {
        mapped.insert("user_location".to_string(), location.clone());
    }
    match provider_type {
        ProviderType::Messages
            if matches!(tool.tool_type.as_str(), "web_search" | "web_search_preview")
                && tool.origin_protocol == Some(urp::ProviderProtocol::Responses) =>
        {
            if let Some(domains) = config
                .and_then(|config| config.get("filters"))
                .and_then(|filters| filters.get("allowed_domains"))
            {
                mapped.insert("allowed_domains".to_string(), domains.clone());
            }
            tool.tool_type = "web_search_20250305".to_string();
            tool.name = Some("web_search".to_string());
            tool.origin_protocol = Some(urp::ProviderProtocol::Messages);
        }
        ProviderType::Responses
            if tool.tool_type.starts_with("web_search_")
                && tool.origin_protocol == Some(urp::ProviderProtocol::Messages) =>
        {
            if let Some(domains) = config.and_then(|config| config.get("allowed_domains")) {
                mapped.insert("filters".to_string(), json!({ "allowed_domains": domains }));
            }
            tool.tool_type = "web_search".to_string();
            tool.name = None;
            tool.origin_protocol = Some(urp::ProviderProtocol::Responses);
        }
        _ => return false,
    }
    tool.description = None;
    tool.config = Some(Value::Object(mapped));
    tool.extra_body.clear();
    true
}

/// T1g.1: a mapped definition and the forced choice selecting it move together.
pub(in crate::handlers) fn map_web_search_tools_for_provider(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
) {
    let Some(tools) = req.tools.as_mut() else {
        return;
    };
    let mut mapped_names = Vec::new();
    for tool in tools.iter_mut() {
        let original_name = tool.name.clone();
        if map_web_search_tool_definition(tool, provider_type) {
            mapped_names.push(original_name);
        }
    }
    if mapped_names.is_empty() {
        return;
    }
    let Some(urp::ToolChoice::Specific(Value::Object(selector))) = &req.tool_choice else {
        return;
    };
    let selects_mapped_tool = match provider_type {
        ProviderType::Messages => matches!(
            selector.get("type").and_then(Value::as_str),
            Some("web_search" | "web_search_preview")
        ),
        _ => {
            selector.get("type").and_then(Value::as_str) == Some("function")
                && selector_name(selector, "function")
                    .is_some_and(|name| mapped_names.iter().flatten().any(|mapped| mapped == name))
        }
    };
    if selects_mapped_tool {
        req.tool_choice = Some(urp::ToolChoice::Specific(match provider_type {
            ProviderType::Messages => {
                json!({ "type": "function", "function": { "name": "web_search" } })
            }
            _ => json!({ "type": "web_search" }),
        }));
    }
}

pub(in crate::handlers) fn filter_tools_for_provider(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
    downstream: DownstreamProtocol,
) {
    map_web_search_tools_for_provider(req, provider_type);
    let Some(tools) = req.tools.as_mut() else {
        if matches!(req.tool_choice, Some(urp::ToolChoice::Specific(_))) {
            req.tool_choice = None;
        }
        return;
    };

    tools.retain(|tool| provider_supports_tool_definition(tool, provider_type, downstream));
    if tools.is_empty() {
        req.tools = None;
        req.tool_choice = None;
        return;
    }

    let Some(choice) = req.tool_choice.as_mut() else {
        return;
    };
    let is_allowed_tools = matches!(
        choice,
        urp::ToolChoice::Specific(Value::Object(obj))
            if obj.get("type").and_then(Value::as_str) == Some("allowed_tools")
    );
    if is_allowed_tools {
        if !matches!(
            provider_type,
            ProviderType::ChatCompletion | ProviderType::Responses | ProviderType::Gemini
        ) {
            req.tool_choice = None;
            return;
        }
        let Some(references) = allowed_tool_references_mut(choice) else {
            req.tool_choice = None;
            return;
        };
        let available = req.tools.as_deref().unwrap_or_default();
        references.retain(|reference| {
            reference.as_object().is_some_and(|selector| {
                available
                    .iter()
                    .any(|tool| selector_matches_tool(selector, tool))
            })
        });
        if references.is_empty() {
            req.tool_choice = None;
        }
        return;
    }

    if let urp::ToolChoice::Specific(Value::Object(selector)) = choice
        && !matches!(
            selector.get("type").and_then(Value::as_str),
            Some("auto" | "required" | "any" | "none") | None
        )
        && !req
            .tools
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|tool| selector_matches_tool(selector, tool))
    {
        req.tool_choice = Some(urp::ToolChoice::Mode("auto".to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_native_tools_and_allowed_lists_survive_provider_filtering() {
        let mut request = urp::decode::gemini::decode_request(&json!({"contents":[],"tools":[
            {"computerUse":{"environment":"ENVIRONMENT_BROWSER"}}, {"googleSearch":{}},
            {"functionDeclarations":[{"name":"run","parametersJsonSchema":{"type":"object"}}]}],
            "toolConfig":{"functionCallingConfig":{"mode":"VALIDATED","allowedFunctionNames":["run"]}}})).unwrap();
        filter_tools_for_provider(
            &mut request,
            ProviderType::Gemini,
            DownstreamProtocol::Responses,
        );
        assert_eq!(request.tools.as_ref().unwrap().len(), 3);
        assert!(request.tool_choice.is_some());
        assert_eq!(
            urp::encode::gemini::encode_request(&request, "m")["toolConfig"]["functionCallingConfig"],
            json!({"mode":"VALIDATED","allowedFunctionNames":["run"]})
        );
        filter_tools_for_provider(
            &mut request,
            ProviderType::Responses,
            DownstreamProtocol::Responses,
        );
        assert_eq!(request.tools.as_ref().unwrap().len(), 1);
        assert_eq!(request.tools.as_ref().unwrap()[0].tool_type, "function");
    }
}

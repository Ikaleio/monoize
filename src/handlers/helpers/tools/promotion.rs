use super::*;

pub(super) fn tool_wire_name(tool: &urp::ToolDefinition) -> Option<&str> {
    match tool.tool_type.as_str() {
        "function" => tool
            .function
            .as_ref()
            .map(|function| function.name.as_str()),
        "custom" => tool.custom.as_ref().map(|custom| custom.name.as_str()),
        _ => tool.name.as_deref(),
    }
}

fn collect_additional_tool_leaves(value: &Value, output: &mut Vec<Value>) {
    let Some(object) = value.as_object() else {
        return;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("namespace") => {
            if let Some(tools) = object.get("tools").and_then(Value::as_array) {
                for tool in tools {
                    let start = output.len();
                    collect_additional_tool_leaves(tool, output);
                    for leaf in &mut output[start..] {
                        if let Some(namespace) = object.get("name").and_then(Value::as_str) {
                            leaf["namespace"] = json!(namespace);
                        }
                    }
                }
            }
        }
        Some("function" | "custom") => output.push(value.clone()),
        _ => {}
    }
}

fn responses_additional_tool_leaves(req: &urp::UrpRequest) -> Vec<Value> {
    let mut output = Vec::new();
    for node in &req.input {
        let urp::Node::ProviderItem {
            origin_protocol: urp::ProviderProtocol::Responses,
            item_type,
            body,
            ..
        } = node
        else {
            continue;
        };
        if item_type != "additional_tools" {
            continue;
        }
        if let Some(tools) = body.get("tools").and_then(Value::as_array) {
            for tool in tools {
                collect_additional_tool_leaves(tool, &mut output);
            }
        }
    }
    output
}

fn custom_tool_has_messages_input_schema(tool: &urp::ToolDefinition) -> bool {
    tool.custom.as_ref().is_some_and(|custom| {
        custom.extra_body.contains_key("input_schema")
            || tool.extra_body.contains_key("input_schema")
    })
}

fn messages_custom_bridge_function(tool: urp::ToolDefinition) -> urp::ToolDefinition {
    let custom = tool
        .custom
        .expect("custom tool promotion requires a custom definition");
    urp::ToolDefinition {
        namespace: None,
        tools: None,
        origin_protocol: None,
        config: None,

        tool_type: "function".to_string(),
        name: None,
        description: None,
        function: Some(urp::FunctionDefinition {
            response_schema: None,
            name: custom.name,
            description: custom.description,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"],
                "additionalProperties": false
            })),
            strict: None,
            extra_body: HashMap::new(),
        }),
        custom: None,
        extra_body: HashMap::new(),
        cache_breakpoint: None,
    }
}

fn tool_call_type(tool: &urp::ToolDefinition) -> Option<urp::ToolCallType> {
    match tool.tool_type.as_str() {
        "function" => Some(urp::ToolCallType::Function),
        "custom" => Some(urp::ToolCallType::Custom),
        _ => None,
    }
}

pub(super) fn active_tool_transports(
    req: &urp::UrpRequest,
) -> impl Iterator<Item = (&str, &urp::ToolTransport)> {
    req.tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|tool| {
            let name = tool_wire_name(tool)?;
            let transport = req.context.tool_transports.get(name)?;
            (tool.namespace.is_none() && tool_call_type(tool) == Some(transport.wire_type))
                .then_some((name, transport))
        })
}

pub(in crate::handlers::helpers) fn messages_custom_bridge_names(
    req: &urp::UrpRequest,
) -> HashSet<String> {
    // Messages and Gemini share the `{"input": string}` function bridge for custom tools.
    active_tool_transports(req)
        .filter(|(_, transport)| {
            matches!(
                transport.protocol,
                urp::ProviderProtocol::Messages | urp::ProviderProtocol::Gemini
            ) && transport.original.tool_type == urp::ToolCallType::Custom
                && transport.wire_type == urp::ToolCallType::Function
        })
        .map(|(name, _)| name.to_owned())
        .collect()
}

fn bridge_messages_custom_history(req: &mut urp::UrpRequest) {
    let names = messages_custom_bridge_names(req);
    if names.is_empty() {
        return;
    }

    let mut bridged_call_ids = HashSet::new();
    for node in &mut req.input {
        let urp::Node::ToolCall {
            tool_type,
            call_id,
            name,
            arguments,
            ..
        } = node
        else {
            continue;
        };
        if *tool_type == urp::ToolCallType::Custom && names.contains(name) {
            *tool_type = urp::ToolCallType::Function;
            *arguments = json!({ "input": arguments.clone() }).to_string();
            bridged_call_ids.insert(call_id.clone());
        }
    }
    for node in &mut req.input {
        let urp::Node::ToolResult {
            tool_type, call_id, ..
        } = node
        else {
            continue;
        };
        if *tool_type == urp::ToolCallType::Custom && bridged_call_ids.contains(call_id) {
            *tool_type = urp::ToolCallType::Function;
        }
    }

    let Some(urp::ToolChoice::Specific(Value::Object(selector))) = req.tool_choice.as_mut() else {
        return;
    };
    if selector.get("type").and_then(Value::as_str) != Some("custom") {
        return;
    }
    let Some(name) = selector_name(selector, "custom")
        .filter(|name| names.contains(*name))
        .map(ToOwned::to_owned)
    else {
        return;
    };
    *selector = serde_json::Map::from_iter([
        ("type".to_string(), Value::String("function".to_string())),
        ("function".to_string(), json!({ "name": name })),
    ]);
}

fn append_explicit_tool(tool: urp::ToolDefinition, output: &mut Vec<urp::ToolDefinition>) {
    if tool.tool_type == "namespace" {
        let namespace = tool.name;
        for mut child in tool.tools.unwrap_or_default() {
            if child.namespace.is_none() {
                child.namespace = namespace.clone();
            }
            append_explicit_tool(child, output);
        }
    } else {
        output.push(tool);
    }
}

pub(in crate::handlers) fn promote_responses_additional_tools(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
) {
    let protocol = match provider_type {
        ProviderType::ChatCompletion => urp::ProviderProtocol::ChatCompletion,
        ProviderType::Messages => urp::ProviderProtocol::Messages,
        ProviderType::Gemini => urp::ProviderProtocol::Gemini,
        _ => return,
    };
    let previous_transports = std::mem::take(&mut req.context.tool_transports);

    let had_tools = req.tools.is_some();
    let mut candidates = Vec::new();
    for tool in req.tools.take().unwrap_or_default() {
        append_explicit_tool(tool, &mut candidates);
    }
    candidates.extend(
        responses_additional_tool_leaves(req)
            .iter()
            .filter_map(urp::decode::parse_tool_definition),
    );
    let mut names: HashSet<String> = candidates
        .iter()
        .filter(|tool| tool.namespace.is_none())
        .filter_map(|tool| tool_wire_name(tool).map(ToOwned::to_owned))
        .collect();
    let mut identities = HashSet::new();
    let mut promoted = Vec::new();
    for mut tool in candidates {
        if !matches!(tool.tool_type.as_str(), "function" | "custom") {
            promoted.push(tool);
            continue;
        }
        let Some(name) = tool_wire_name(&tool).map(ToOwned::to_owned) else {
            promoted.push(tool);
            continue;
        };
        let namespace = tool.namespace.take();
        let original_type = tool_call_type(&tool).expect("client tool type was checked");
        let existing_transport = previous_transports.get(&name).filter(|transport| {
            transport.protocol == protocol
                && namespace.is_none()
                && transport.wire_type == original_type
        });
        let original = existing_transport
            .map(|transport| transport.original.clone())
            .unwrap_or_else(|| urp::ToolIdentity {
                namespace: namespace.clone(),
                name: name.clone(),
                tool_type: original_type,
            });
        if !identities.insert((original.namespace.clone(), original.name.clone())) {
            continue;
        }
        if let Some(namespace) = namespace {
            let prefix: String = format!("{namespace}_{name}")
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .take(40)
                .collect();
            let mut index = promoted.len();
            let alias = loop {
                let candidate = format!("{prefix}_{index}");
                if names.insert(candidate.clone()) {
                    break candidate;
                }
                index += 1;
            };
            if let Some(function) = &mut tool.function {
                function.name = alias.clone();
            }
            if let Some(custom) = &mut tool.custom {
                custom.name = alias;
            }
        }
        if (provider_type == ProviderType::Messages
            && tool.tool_type == "custom"
            && !custom_tool_has_messages_input_schema(&tool))
            || (provider_type == ProviderType::Gemini && tool.tool_type == "custom")
        {
            tool = messages_custom_bridge_function(tool);
        }
        let wire_type = tool_call_type(&tool).expect("adapted client tool type");
        if let Some(transport) = existing_transport {
            req.context.tool_transports.insert(name, transport.clone());
        } else if original.namespace.is_some() || wire_type != original_type {
            req.context.tool_transports.insert(
                tool_wire_name(&tool).expect("adapted tool name").to_owned(),
                urp::ToolTransport {
                    protocol,
                    wire_type,
                    original,
                },
            );
        }
        promoted.push(tool);
    }
    req.tools = (had_tools || !promoted.is_empty()).then_some(promoted);
    bridge_namespaced_history(req);
    if matches!(provider_type, ProviderType::Messages | ProviderType::Gemini) {
        bridge_messages_custom_history(req);
    }
    if provider_type == ProviderType::Gemini {
        bridge_gemini_custom_history(req);
    }
}

fn bridge_namespaced_history(req: &mut urp::UrpRequest) {
    let aliases = tool_namespace_aliases(req);
    let mut call_aliases = HashMap::new();
    for node in &mut req.input {
        if let urp::Node::ToolCall {
            call_id,
            name,
            namespace,
            ..
        } = node
            && let Some(current_namespace) = namespace.as_deref()
            && let Some((alias, _)) = aliases.iter().find(|(_, identity)| {
                identity.namespace.as_deref() == Some(current_namespace) && identity.name == *name
            })
        {
            call_aliases.insert(call_id.clone(), alias.clone());
            *name = alias.clone();
            *namespace = None;
        }
    }
    for node in &mut req.input {
        if let urp::Node::ToolResult {
            call_id,
            name,
            namespace,
            ..
        } = node
        {
            let alias = match (namespace.as_deref(), name.as_deref()) {
                (Some(namespace), Some(name)) => aliases.iter().find_map(|(alias, identity)| {
                    (identity.namespace.as_deref() == Some(namespace) && identity.name == name)
                        .then_some(alias)
                }),
                (None, Some(_)) => call_aliases.get(call_id),
                _ => None,
            };
            if let Some(alias) = alias {
                *name = Some(alias.clone());
                *namespace = None;
            }
        }
    }
    if let Some(urp::ToolChoice::Specific(choice)) = &mut req.tool_choice {
        bridge_namespace_selector(choice, &aliases);
    }
}

fn bridge_gemini_custom_history(req: &mut urp::UrpRequest) {
    // Gemini has no custom call type: undeclared custom history uses the same `{input}`
    // function shape so the conversation stays encodable.
    for node in &mut req.input {
        match node {
            urp::Node::ToolCall {
                tool_type,
                arguments,
                ..
            } if *tool_type == urp::ToolCallType::Custom => {
                *tool_type = urp::ToolCallType::Function;
                *arguments = json!({ "input": arguments.clone() }).to_string();
            }
            urp::Node::ToolResult { tool_type, .. } if *tool_type == urp::ToolCallType::Custom => {
                *tool_type = urp::ToolCallType::Function;
            }
            _ => {}
        }
    }
}

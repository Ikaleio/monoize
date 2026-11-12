use super::*;

pub(in crate::handlers) fn tool_namespace_aliases(
    req: &urp::UrpRequest,
) -> HashMap<String, urp::ToolIdentity> {
    active_tool_transports(req)
        .filter(|(_, transport)| transport.original.namespace.is_some())
        .map(|(name, transport)| (name.to_owned(), transport.original.clone()))
        .collect()
}

pub(super) fn bridge_namespace_selector(
    value: &mut Value,
    aliases: &HashMap<String, urp::ToolIdentity>,
) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if matches!(kind.as_str(), "function" | "custom") {
        let namespace = obj
            .get("namespace")
            .or_else(|| obj.get(&kind)?.get("namespace"));
        if let Some(namespace) = namespace {
            let name = selector_name(obj, &kind);
            if let Some((alias, _)) = aliases.iter().find(|(_, identity)| {
                identity.namespace.as_deref() == namespace.as_str()
                    && Some(identity.name.as_str()) == name
            }) {
                obj.remove("namespace");
                obj.remove("name");
                obj.insert(kind, json!({"name": alias}));
            }
        }
    } else if kind == "allowed_tools" {
        let tools = if obj.contains_key("allowed_tools") {
            obj.get_mut("allowed_tools")
                .and_then(|wrapper| wrapper.get_mut("tools"))
        } else {
            obj.get_mut("tools")
        };
        if let Some(tools) = tools.and_then(Value::as_array_mut) {
            for tool in tools {
                bridge_namespace_selector(tool, aliases);
            }
        }
    }
}

fn restore_tool_namespace(
    name: &mut String,
    target_namespace: &mut Option<String>,
    aliases: &HashMap<String, urp::ToolIdentity>,
) {
    if let Some(identity) = aliases.get(name) {
        *name = identity.name.clone();
        *target_namespace = identity.namespace.clone();
    }
}

pub(in crate::handlers) fn restore_tool_namespace_node(
    node: &mut urp::Node,
    aliases: &HashMap<String, urp::ToolIdentity>,
) {
    if let urp::Node::ToolCall {
        name, namespace, ..
    } = node
    {
        restore_tool_namespace(name, namespace, aliases);
    }
}

pub(in crate::handlers) fn restore_tool_namespace_event(
    event: &mut urp::UrpStreamEvent,
    aliases: &HashMap<String, urp::ToolIdentity>,
) {
    match event {
        urp::UrpStreamEvent::NodeStart {
            header: urp::NodeHeader::ToolCall {
                name, namespace, ..
            },
            ..
        } => {
            restore_tool_namespace(name, namespace, aliases);
        }
        urp::UrpStreamEvent::NodeDone { node, .. } => restore_tool_namespace_node(node, aliases),
        urp::UrpStreamEvent::ResponseDone { output, .. } => {
            for entry in output {
                restore_tool_namespace_node(&mut entry.node, aliases);
            }
        }
        _ => {}
    }
}

pub(in crate::handlers) fn restore_messages_custom_tool_calls(
    request: &urp::UrpRequest,
    response: &mut urp::UrpResponse,
) {
    let names = messages_custom_bridge_names(request);
    if names.is_empty() {
        return;
    }
    for node in &mut response.output {
        let urp::Node::ToolCall {
            tool_type,
            name,
            arguments,
            ..
        } = node
        else {
            continue;
        };
        if *tool_type != urp::ToolCallType::Function || !names.contains(name) {
            continue;
        }
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(arguments) else {
            continue;
        };
        let Some(input) = object.get("input").and_then(Value::as_str) else {
            continue;
        };
        *tool_type = urp::ToolCallType::Custom;
        *arguments = input.to_string();
    }
}

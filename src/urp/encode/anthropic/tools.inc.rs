fn prepare_schema_custom_tools(req: &mut UrpRequest) -> Result<(), String> {
    let mut identities = std::collections::HashSet::new();
    for tool in req.tools.iter_mut().flatten() {
        if tool.tool_type != "custom" {
            continue;
        }
        let Some(custom) = &mut tool.custom else {
            continue;
        };
        let Some(schema) = custom
            .extra_body
            .remove("input_schema")
            .or_else(|| tool.extra_body.remove("input_schema"))
        else {
            continue;
        };
        tool.extra_body.remove("input_schema");
        if !schema.is_object() || schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(format!(
                "Messages custom tool {} requires an object input_schema",
                custom.name
            ));
        }
        identities.insert((tool.namespace.clone(), custom.name.clone()));
        let custom = tool.custom.take().unwrap();
        tool.tool_type = "function".to_string();
        tool.function = Some(crate::urp::FunctionDefinition {
            name: custom.name,
            description: custom.description,
            parameters: Some(schema),
            strict: None,
            extra_body: custom.extra_body,
        });
    }
    if identities.is_empty() {
        return Ok(());
    }
    let mut call_ids = std::collections::HashSet::new();
    for node in &mut req.input {
        if let Node::ToolCall {
            tool_type,
            namespace,
            name,
            call_id,
            arguments,
            ..
        } = node
            && *tool_type == ToolCallType::Custom
            && identities.contains(&(namespace.clone(), name.clone()))
        {
            if !serde_json::from_str::<Value>(arguments).is_ok_and(|value| value.is_object()) {
                return Err(format!(
                    "Messages custom tool {name} input must be a complete JSON object"
                ));
            }
            *tool_type = ToolCallType::Function;
            call_ids.insert(call_id.clone());
        }
    }
    for node in &mut req.input {
        if let Node::ToolResult {
            tool_type, call_id, ..
        } = node
            && *tool_type == ToolCallType::Custom
            && call_ids.contains(call_id)
        {
            *tool_type = ToolCallType::Function;
        }
    }
    if let Some(crate::urp::ToolChoice::Specific(Value::Object(choice))) = &mut req.tool_choice
        && choice.get("type").and_then(Value::as_str) == Some("custom")
    {
        let name = choice
            .get("name")
            .or_else(|| choice.get("custom").and_then(|custom| custom.get("name")))
            .and_then(Value::as_str);
        let namespace = choice
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(name) = name
            && identities.contains(&(namespace, name.to_string()))
        {
            *choice = Map::from_iter([
                ("type".to_string(), json!("function")),
                ("function".to_string(), json!({"name": name})),
            ]);
        }
    }
    Ok(())
}

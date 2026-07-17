fn encode_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut item = Map::new();
                item.insert("name".to_string(), Value::String(function.name.clone()));
                if let Some(description) = &function.description {
                    item.insert(
                        "description".to_string(),
                        Value::String(description.clone()),
                    );
                }
                item.insert(
                    "input_schema".to_string(),
                    function.parameters.clone().unwrap_or(json!({
                        "type": "object",
                        "properties": {},
                        "additionalProperties": true
                    })),
                );
                if let Some(strict) = function.strict {
                    item.insert("strict".to_string(), Value::Bool(strict));
                }
                merge_extra(&mut item, &function.extra_body);
                merge_extra(&mut item, &tool.extra_body);
                out.push(Value::Object(item));
            }
        } else if tool.tool_type == "custom" {
            if let Some(custom) = &tool.custom {
                let mut item = Map::new();
                item.insert("type".to_string(), Value::String("custom".to_string()));
                item.insert("name".to_string(), Value::String(custom.name.clone()));
                if let Some(description) = &custom.description {
                    item.insert(
                        "description".to_string(),
                        Value::String(description.clone()),
                    );
                }
                if let Some(format) = &custom.format {
                    item.insert("format".to_string(), format.clone());
                }
                merge_extra(&mut item, &custom.extra_body);
                merge_extra(&mut item, &tool.extra_body);
                out.push(Value::Object(item));
            }
        } else {
            let mut item = Map::new();
            item.insert("type".to_string(), Value::String(tool.tool_type.clone()));
            if let Some(name) = &tool.name {
                item.insert("name".to_string(), Value::String(name.clone()));
            }
            if let Some(description) = &tool.description {
                item.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            merge_extra(&mut item, &tool.extra_body);
            out.push(Value::Object(item));
        }
    }
    out
}

fn encode_tool_choice_for_anthropic(
    choice: &crate::urp::ToolChoice,
    parallel_tool_calls: Option<bool>,
) -> Value {
    match tool_choice_to_value(choice) {
        Value::String(mode) => match mode.as_str() {
            "auto" => anthropic_tool_choice_object("auto", None, parallel_tool_calls),
            "required" => anthropic_tool_choice_object("any", None, parallel_tool_calls),
            "none" => json!({ "type": "none" }),
            _ => Value::String(mode),
        },
        Value::Object(obj) => {
            let explicit_disable = obj
                .get("disable_parallel_tool_use")
                .and_then(|v| v.as_bool());
            if let Some(name) = obj
                .get("function")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                let mut out = Map::new();
                out.insert("type".to_string(), Value::String("tool".to_string()));
                out.insert("name".to_string(), Value::String(name.to_string()));
                insert_anthropic_disable_parallel(&mut out, explicit_disable, parallel_tool_calls);
                Value::Object(out)
            } else if let Some(mode) = obj.get("type").and_then(|v| v.as_str()) {
                match mode {
                    "auto" => {
                        anthropic_tool_choice_object("auto", explicit_disable, parallel_tool_calls)
                    }
                    "required" | "any" => {
                        anthropic_tool_choice_object("any", explicit_disable, parallel_tool_calls)
                    }
                    "none" => json!({ "type": "none" }),
                    _ => Value::Object(obj),
                }
            } else {
                Value::Object(obj)
            }
        }
        other => other,
    }
}

fn anthropic_tool_choice_object(
    mode: &str,
    explicit_disable: Option<bool>,
    parallel_tool_calls: Option<bool>,
) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String(mode.to_string()));
    insert_anthropic_disable_parallel(&mut obj, explicit_disable, parallel_tool_calls);
    Value::Object(obj)
}

fn insert_anthropic_disable_parallel(
    obj: &mut Map<String, Value>,
    explicit_disable: Option<bool>,
    parallel_tool_calls: Option<bool>,
) {
    let disable = explicit_disable.or_else(|| (parallel_tool_calls == Some(false)).then_some(true));
    if let Some(disable) = disable {
        obj.insert(
            "disable_parallel_tool_use".to_string(),
            Value::Bool(disable),
        );
    }
}

fn encode_anthropic_image(source: &ImageSource) -> Value {
    match source {
        ImageSource::Url { url, .. } => json!({
            "type": "image",
            "source": { "type": "url", "url": url }
        }),
        ImageSource::Base64 { media_type, data } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data }
        }),
    }
}

fn encode_anthropic_file(source: &FileSource) -> Value {
    match source {
        FileSource::Url { url } => json!({
            "type": "document",
            "source": { "type": "url", "url": url }
        }),
        FileSource::Base64 {
            filename,
            media_type,
            data,
        } => json!({
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data,
                "filename": filename
            }
        }),
    }
}

/// Claude identifiers default to adaptive thinking. The bounded legacy
/// exceptions are Opus <= 4.6, Sonnet <= 4.6, and Haiku <= 4.5.
fn model_supports_adaptive(model: &str) -> bool {
    let m = model.to_lowercase();
    let denotes_claude = m.contains("claude")
        || ["opus-", "sonnet-", "haiku-"]
            .iter()
            .any(|prefix| m.starts_with(prefix));
    if !denotes_claude {
        return false;
    }

    for (family, last_non_adaptive) in [
        ("opus", (4, 6)),
        ("sonnet", (4, 6)),
        ("haiku", (4, 5)),
    ] {
        if let Some(version) = claude_family_version(&m, family) {
            return version > last_non_adaptive;
        }
    }

    true
}

fn claude_family_version(model: &str, family: &str) -> Option<(u32, u32)> {
    let family_pos = model.find(family)?;
    if let Some(version) = parse_model_version_prefix(&model[family_pos + family.len()..]) {
        return Some(version);
    }

    let claude_pos = model.find("claude")?;
    if claude_pos < family_pos {
        return parse_model_version_prefix(&model[claude_pos + "claude".len()..]);
    }
    None
}

fn parse_model_version_prefix(value: &str) -> Option<(u32, u32)> {
    let value = value.trim_start_matches(['-', '.']);
    let major_len = value.chars().take_while(|c| c.is_ascii_digit()).count();
    if major_len == 0 {
        return None;
    }
    let major = value[..major_len].parse::<u32>().ok()?;
    let rest = value[major_len..]
        .strip_prefix('-')
        .or_else(|| value[major_len..].strip_prefix('.'));
    let minor = rest
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|minor| !minor.is_empty())
        .and_then(|minor| minor.parse::<u32>().ok())
        .unwrap_or(0);
    Some((major, minor))
}

fn effort_to_budget(effort: &str) -> u32 {
    // Non-adaptive Anthropic models use a fixed budget table. `xhigh` and `max`
    // share the same budget here; their distinction only surfaces on
    // adaptive-thinking models via `output_config.effort`.
    match effort {
        "minimum" => 1024,
        "low" => 1024,
        "medium" => 4096,
        "high" => 16384,
        "xhigh" | "max" => 32000,
        _ => 4096,
    }
}

fn finish_reason_to_stop_reason(finish_reason: Option<FinishReason>) -> &'static str {
    match finish_reason {
        Some(FinishReason::Length) => "max_tokens",
        Some(FinishReason::ToolCalls) => "tool_use",
        _ => "end_turn",
    }
}

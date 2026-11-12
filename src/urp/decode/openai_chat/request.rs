use super::message::{
    MESSAGE_BODY_FIELDS, attach_annotations, legacy_function_call_id, legacy_function_call_node,
    message_audio_node, push_content_nodes, push_message, refusal_node, tool_call_nodes,
};
use super::reasoning::reasoning_nodes;
use crate::urp::decode::{
    normalize_reasoning_effort, parse_compatible_media_node, parse_tool_definition,
    remove_untrusted_internal_keys, split_extra,
};
use crate::urp::{
    AudioSource, CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY, CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY,
    CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY, CHAT_MAX_TOKENS_FIELD_EXTRA_KEY,
    CHAT_MESSAGE_ITEM_EXTRA_KEY, CHAT_REASONING_CONFIG_EXTRA_KEY, CHAT_THINKING_CONFIG_EXTRA_KEY,
    FileSource, JsonSchemaDefinition, Node, OrdinaryRole, ProviderProtocol, ReasoningConfig,
    ResponseFormat, StopControl, ToolCallType, ToolChoice, ToolDefinition, ToolResultContent,
    UrpRequest, is_internal_key,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    let mut request = decode_native_request(value)?;
    crate::urp::cache_breakpoint::decode_request_markers(
        &mut request,
        ProviderProtocol::ChatCompletion,
    );
    Ok(request)
}

fn decode_native_request(value: &Value) -> Result<UrpRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat request must be object".to_string())?;

    if let Some(n) = obj.get("n").filter(|n| !n.is_null())
        && n.as_u64() != Some(1)
    {
        return Err("Chat Completions n must be null or the integer 1".to_string());
    }

    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing model".to_string())?
        .to_string();

    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing messages".to_string())?;
    let mut input = decode_messages(messages)?;

    let (tool_choice, legacy_function_choice_raw) = decode_tool_choice(obj);

    let mut extra_body = split_extra(
        obj,
        &[
            "model",
            "messages",
            "stream",
            "temperature",
            "top_p",
            "max_completion_tokens",
            "max_tokens",
            "reasoning_effort",
            "reasoning",
            "thinking",
            "tools",
            "functions",
            "tool_choice",
            "function_call",
            "parallel_tool_calls",
            "stop",
            "verbosity",
            "response_format",
            "user",
        ],
    );
    if let Some(raw_choice) = legacy_function_choice_raw {
        extra_body.insert(
            CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY.to_string(),
            raw_choice
                .as_object()
                .map(|obj| Value::Object(split_extra(obj, &["name"]).into_iter().collect()))
                .unwrap_or(Value::Null),
        );
    }
    if let Some(field) = ["max_completion_tokens", "max_tokens"]
        .into_iter()
        .find(|field| obj.get(*field).and_then(Value::as_u64).is_some())
    {
        extra_body.insert(
            CHAT_MAX_TOKENS_FIELD_EXTRA_KEY.to_string(),
            Value::String(field.to_string()),
        );
    }

    crate::urp::tool_signature::restore_request_call_signatures(&mut input);
    crate::urp::logprobs::strip_request_extras(&mut extra_body);
    crate::urp::sampling::strip_request_extras(&mut extra_body, ProviderProtocol::ChatCompletion);
    Ok(UrpRequest {
        image_generation: None,
        sampling: crate::urp::sampling::request_config(obj, ProviderProtocol::ChatCompletion),
        logprobs: crate::urp::logprobs::request_config(obj, ProviderProtocol::ChatCompletion),
        context: Default::default(),
        instructions_format: None,
        model,
        input,
        stream: obj.get("stream").and_then(Value::as_bool),
        temperature: obj.get("temperature").and_then(Value::as_f64),
        top_p: obj.get("top_p").and_then(Value::as_f64),
        max_output_tokens: obj
            .get("max_completion_tokens")
            .or_else(|| obj.get("max_tokens"))
            .and_then(Value::as_u64),
        reasoning: decode_reasoning_config(obj),
        tools: decode_tools(obj),
        tool_choice,
        parallel_tool_calls: obj.get("parallel_tool_calls").and_then(Value::as_bool),
        stop: obj.get("stop").and_then(|value| match value {
            Value::String(stop) => Some(StopControl::Single(stop.clone())),
            Value::Array(stops) => stops
                .iter()
                .map(|stop| stop.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .map(StopControl::Multiple),
            _ => None,
        }),
        verbosity: obj
            .get("verbosity")
            .and_then(Value::as_str)
            .map(str::to_string),
        response_format: obj.get("response_format").and_then(parse_response_format),
        user: obj.get("user").and_then(Value::as_str).map(str::to_string),
        extra_body,
    })
}

fn decode_messages(messages: &[Value]) -> Result<Vec<Node>, String> {
    let mut input = Vec::new();
    // A `tool` message inherits the call type of the assistant call it answers.
    let mut tool_call_types = HashMap::new();
    for raw_msg in messages {
        let Some(msg_obj) = raw_msg.as_object() else {
            continue;
        };
        if msg_obj
            .get("configuration_update")
            .is_some_and(Value::is_object)
        {
            input.push(configuration_update_node(raw_msg, msg_obj));
            continue;
        }
        let role = match msg_obj.get("role").and_then(Value::as_str) {
            Some("function") => {
                input.push(legacy_function_result_node(msg_obj)?);
                continue;
            }
            Some("tool") => {
                input.push(tool_result_node(msg_obj, &tool_call_types)?);
                continue;
            }
            Some("system") => OrdinaryRole::System,
            Some("developer") => OrdinaryRole::Developer,
            Some("assistant") => OrdinaryRole::Assistant,
            _ => OrdinaryRole::User,
        };

        let mut nodes = reasoning_nodes(msg_obj);
        nodes.extend(message_audio_node(msg_obj, role));
        if let Some(content) = msg_obj.get("content") {
            let phase = msg_obj.get("phase").and_then(Value::as_str);
            push_content_nodes(&mut nodes, content, role, phase)?;
        }
        attach_annotations(&mut nodes, msg_obj);
        nodes.extend(refusal_node(msg_obj));
        for tool_call in tool_call_nodes(msg_obj).chain(legacy_function_call_node(msg_obj)) {
            if let Node::ToolCall {
                call_id, tool_type, ..
            } = &tool_call
            {
                tool_call_types.insert(call_id.clone(), *tool_type);
            }
            nodes.push(tool_call);
        }

        let mut extra_body = split_extra(msg_obj, MESSAGE_BODY_FIELDS);
        crate::urp::cache_breakpoint::attach_chat_message_marker(&mut nodes, &mut extra_body);
        push_message(&mut input, nodes, extra_body);
    }
    Ok(input)
}

fn configuration_update_node(raw_msg: &Value, msg_obj: &Map<String, Value>) -> Node {
    Node::ProviderItem {
        id: msg_obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
        origin_protocol: ProviderProtocol::ChatCompletion,
        role: OrdinaryRole::System,
        item_type: msg_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("configuration_update")
            .to_string(),
        body: crate::urp::encode::sanitize_provider_item_wire_body(raw_msg),
        extra_body: HashMap::from([(CHAT_MESSAGE_ITEM_EXTRA_KEY.to_string(), Value::Bool(true))]),
    }
}

fn legacy_function_result_node(msg_obj: &Map<String, Value>) -> Result<Node, String> {
    let name = msg_obj
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut extra_body = split_extra(msg_obj, &["role", "id", "name", "content"]);
    extra_body.insert(
        CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    Ok(Node::ToolResult {
        signature: None,
        namespace: None,
        call_id: legacy_function_call_id(name.as_deref().unwrap_or_default()),
        name,
        id: msg_obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_type: ToolCallType::Function,
        is_error: false,
        content: decode_tool_result_content(msg_obj.get("content").unwrap_or(&Value::Null))?,
        cache_breakpoint: None,
        extra_body,
    })
}

fn tool_result_node(
    msg_obj: &Map<String, Value>,
    tool_call_types: &HashMap<String, ToolCallType>,
) -> Result<Node, String> {
    let call_id = msg_obj
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(Node::ToolResult {
        signature: None,
        namespace: None,
        name: msg_obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        id: msg_obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_type: tool_call_types
            .get(&call_id)
            .copied()
            .unwrap_or(ToolCallType::Function),
        call_id,
        is_error: false,
        content: decode_tool_result_content(msg_obj.get("content").unwrap_or(&Value::Null))?,
        cache_breakpoint: None,
        extra_body: split_extra(msg_obj, &["role", "id", "name", "tool_call_id", "content"]),
    })
}

fn decode_tool_result_content(value: &Value) -> Result<Vec<ToolResultContent>, String> {
    let values = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(values) => values.as_slice(),
        _ => std::slice::from_ref(value),
    };
    let mut content = Vec::new();
    for value in values {
        if let Some(obj) = value.as_object() {
            let kind = obj.get("type").and_then(Value::as_str);
            if matches!(kind, Some("input_text" | "output_text" | "text"))
                && let Some(text) = obj
                    .get("text")
                    .or_else(|| obj.get("content"))
                    .and_then(Value::as_str)
            {
                content.push(ToolResultContent::Text {
                    text: text.into(),
                    extra_body: split_extra(obj, &["type", "text", "content"]),
                    cache_breakpoint: None,
                });
                continue;
            }
            if let Some(media) = parse_compatible_media_node(obj, OrdinaryRole::User)? {
                content.push(tool_result_media(media));
                continue;
            }
        }
        content.push(ToolResultContent::Text {
            text: value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
            extra_body: HashMap::new(),
            cache_breakpoint: None,
        });
    }
    Ok(content)
}

/// Tool results carry no audio variant; audio travels as a file with the same source.
fn tool_result_media(node: Node) -> ToolResultContent {
    match node {
        Node::Image {
            metadata,
            source,
            extra_body,
            ..
        } => ToolResultContent::Image {
            metadata,
            source,
            extra_body,
        },
        Node::File {
            metadata,
            source,
            extra_body,
            ..
        } => ToolResultContent::File {
            metadata,
            source,
            extra_body,
        },
        Node::Audio {
            metadata,
            source,
            extra_body,
            ..
        } => ToolResultContent::File {
            metadata,
            source: match source {
                AudioSource::Base64 { media_type, data } => FileSource::Base64 { media_type, data },
                AudioSource::Url { url } => FileSource::Url { url },
            },
            extra_body,
        },
        _ => unreachable!("parse_compatible_media_node yields only media nodes"),
    }
}

fn decode_tools(obj: &Map<String, Value>) -> Option<Vec<ToolDefinition>> {
    let modern_tools = obj.get("tools").and_then(Value::as_array);
    let legacy_functions = obj.get("functions").and_then(Value::as_array);
    if modern_tools.is_none() && legacy_functions.is_none() {
        return None;
    }
    Some(
        modern_tools
            .into_iter()
            .flatten()
            .filter_map(|raw| {
                let mut tool = parse_tool_definition(raw)?;
                if tool.function.is_some() || tool.tool_type.starts_with("openrouter:") {
                    tool.origin_protocol = Some(ProviderProtocol::ChatCompletion);
                }
                Some(tool)
            })
            .chain(
                legacy_functions
                    .into_iter()
                    .flatten()
                    .filter_map(legacy_function_definition),
            )
            .collect(),
    )
}

fn legacy_function_definition(value: &Value) -> Option<ToolDefinition> {
    let wrapper = Value::Object(Map::from_iter([
        ("type".to_string(), Value::String("function".to_string())),
        ("function".to_string(), value.clone()),
    ]));
    let mut tool = parse_tool_definition(&wrapper)?;
    tool.origin_protocol = Some(ProviderProtocol::ChatCompletion);
    tool.extra_body.insert(
        CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    Some(tool)
}

/// `tool_choice` wins over the deprecated `function_call`; the latter also returns its
/// sanitized raw value so unknown members survive.
fn decode_tool_choice(obj: &Map<String, Value>) -> (Option<ToolChoice>, Option<Value>) {
    if let Some(choice) = obj.get("tool_choice").filter(|choice| !choice.is_null()) {
        let mut choice = choice.clone();
        remove_untrusted_internal_keys(&mut choice);
        let choice = match choice {
            Value::String(mode) => ToolChoice::Mode(mode),
            choice => ToolChoice::Specific(choice),
        };
        return (Some(choice), None);
    }
    let Some(raw) = obj.get("function_call").filter(|choice| !choice.is_null()) else {
        return (None, None);
    };
    let Some(choice) = legacy_function_choice(raw) else {
        return (None, None);
    };
    let mut raw = raw.clone();
    remove_untrusted_internal_keys(&mut raw);
    (Some(choice), Some(raw))
}

fn legacy_function_choice(value: &Value) -> Option<ToolChoice> {
    if let Some(mode) = value.as_str() {
        return Some(ToolChoice::Mode(mode.to_string()));
    }
    let name = value.as_object()?.get("name")?.as_str()?;
    Some(ToolChoice::Specific(serde_json::json!({
        "type": "function",
        "function": { "name": name }
    })))
}

fn decode_reasoning_config(obj: &Map<String, Value>) -> Option<ReasoningConfig> {
    let reasoning_obj = obj.get("reasoning").and_then(Value::as_object);
    let thinking_obj = obj.get("thinking").and_then(Value::as_object);
    let effort = reasoning_obj
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .or_else(|| obj.get("reasoning_effort").and_then(Value::as_str))
        .map(normalize_reasoning_effort);

    if effort.is_none() && reasoning_obj.is_none() && thinking_obj.is_none() {
        return None;
    }

    let mut extra_body = HashMap::new();
    if let Some(reasoning) = reasoning_obj {
        let mut reasoning = reasoning.clone();
        reasoning.retain(|key, _| {
            !is_internal_key(key)
                && !matches!(
                    key.as_str(),
                    "effort" | "summary" | "max_tokens" | "enabled"
                )
        });
        extra_body.insert(
            CHAT_REASONING_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(reasoning),
        );
    }
    if let Some(thinking) = thinking_obj {
        let mut thinking = thinking.clone();
        thinking.retain(|key, _| {
            !is_internal_key(key) && !matches!(key.as_str(), "type" | "budget_tokens" | "display")
        });
        extra_body.insert(
            CHAT_THINKING_CONFIG_EXTRA_KEY.to_string(),
            Value::Object(thinking),
        );
    }
    Some(ReasoningConfig {
        effort,
        summary: reasoning_obj
            .and_then(|v| v.get("summary"))
            .and_then(Value::as_str)
            .map(str::to_string),
        mode: thinking_obj
            .and_then(|v| v.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                reasoning_obj
                    .and_then(|v| v.get("enabled"))
                    .and_then(Value::as_bool)
                    .map(|enabled| if enabled { "enabled" } else { "disabled" }.to_string())
            }),
        budget_tokens: thinking_obj
            .and_then(|v| v.get("budget_tokens"))
            .or_else(|| reasoning_obj.and_then(|v| v.get("max_tokens")))
            .and_then(Value::as_u64),
        display: thinking_obj
            .and_then(|v| v.get("display"))
            .and_then(Value::as_str)
            .map(str::to_string),
        extra_body,
    })
}

fn parse_response_format(value: &Value) -> Option<ResponseFormat> {
    match value {
        Value::String(format) => match format.as_str() {
            "json_object" => Some(ResponseFormat::JsonObject),
            "text" => Some(ResponseFormat::Text),
            _ => None,
        },
        Value::Object(obj) => match obj.get("type").and_then(Value::as_str)? {
            "text" => Some(ResponseFormat::Text),
            "json_object" => Some(ResponseFormat::JsonObject),
            "json_schema" => {
                let schema_obj = obj.get("json_schema")?.as_object()?;
                Some(ResponseFormat::JsonSchema {
                    json_schema: JsonSchemaDefinition {
                        name: schema_obj.get("name")?.as_str()?.to_string(),
                        description: schema_obj
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        schema: schema_obj.get("schema").cloned().unwrap_or(Value::Null),
                        strict: schema_obj.get("strict").and_then(Value::as_bool),
                        extra_body: split_extra(
                            schema_obj,
                            &["name", "schema", "description", "strict"],
                        ),
                    },
                })
            }
            _ => None,
        },
        _ => None,
    }
}

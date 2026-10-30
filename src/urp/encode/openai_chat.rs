use crate::urp::encode::{
    role_to_str, sanitize_provider_item_wire_body, text_parts, tool_choice_to_chat_value,
    usage_input_details, usage_output_details,
};
use crate::urp::internal_legacy_bridge::{Item, Part, Role, nodes_to_items};
use crate::urp::{
    AudioSource, CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY, CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY,
    CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY, CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY,
    CHAT_MESSAGE_AUDIO_EXTRA_KEY, CHAT_REASONING_CONFIG_EXTRA_KEY, CHAT_REASONING_DETAIL_EXTRA_KEY,
    CHAT_REASONING_SURFACE_EXTRA_KEY, CHAT_REASONING_SURFACE_REASONING_CONTENT,
    CHAT_REASONING_SURFACE_REASONING_OPAQUE, CHAT_THINKING_CONFIG_EXTRA_KEY, FileSource,
    FinishReason, ImageSource, Node, OrdinaryRole, ProviderProtocol, ResponseFormat, StopControl,
    ToolCallType, ToolChoice, ToolDefinition, ToolResultContent, UrpRequest, UrpResponse,
    tool_call_arguments_for_wire,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const CHAT_CHOICE_EXTRA_BODY_KEY: &str = "_monoize_chat_choice_extra";
const CHAT_NATIVE_FINISH_REASON_EXTRA_KEY: &str = "_monoize_chat_native_finish_reason";

struct PendingChatMessage {
    role: Role,
    content_parts: Vec<Value>,
    tool_calls: Vec<Value>,
    refusal: Option<String>,
    reasoning_parts: Vec<Part>,
    legacy_function_call: Option<Value>,
    message_extra: HashMap<String, Value>,
}

fn encode_chat_tool_call(
    tool_type: ToolCallType,
    call_id: &str,
    name: &str,
    arguments: &str,
) -> Value {
    match tool_type {
        ToolCallType::Function => json!({
            "id": call_id,
            "type": "function",
            "function": { "name": name, "arguments": tool_call_arguments_for_wire(arguments) }
        }),
        ToolCallType::Custom => json!({
            "id": call_id,
            "type": "custom",
            "custom": { "name": name, "input": arguments }
        }),
    }
}

/// DC7b: the Chat completion id for both modes; generated only when the URP id is empty.
pub(crate) fn chat_completion_id(id: &str) -> String {
    if id.is_empty() {
        format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
    } else {
        id.to_string()
    }
}

/// DC7a: the Chat `usage` object for non-stream responses and the stream usage chunk.
pub(crate) fn encode_chat_usage(usage: &crate::urp::Usage) -> Value {
    let aggregate = usage.accounting();
    let usage = aggregate.as_ref();
    let input_details = usage_input_details(usage);
    let output_details = usage_output_details(usage);
    let mut prompt_details = Map::from_iter([(
        "cached_tokens".to_string(),
        json!(input_details.cache_read_tokens),
    )]);
    // Decision 4: counters outside the OpenAI schema appear only when non-zero.
    for (key, value) in [
        ("cache_write_tokens", input_details.cache_creation_tokens),
        ("cache_creation_tokens", input_details.cache_creation_tokens),
        ("tool_prompt_tokens", input_details.tool_prompt_tokens),
    ] {
        if value > 0 {
            prompt_details.insert(key.to_string(), json!(value));
        }
    }
    let mut usage_value = json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens(),
        "completion_tokens_details": {
            "reasoning_tokens": output_details.reasoning_tokens,
            "accepted_prediction_tokens": output_details.accepted_prediction_tokens,
            "rejected_prediction_tokens": output_details.rejected_prediction_tokens
        },
        "prompt_tokens_details": prompt_details,
    });
    let usage_obj = usage_value.as_object_mut().expect("chat usage object");
    for detail_key in ["prompt_tokens_details", "completion_tokens_details"] {
        let Some(extra_detail) = usage.extra_body.get(detail_key).and_then(Value::as_object)
        else {
            continue;
        };
        let Some(generated_detail) = usage_obj.get_mut(detail_key).and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (key, value) in extra_detail {
            if !key.starts_with("_monoize_") {
                generated_detail
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    for (key, value) in &usage.extra_body {
        if !key.starts_with("_monoize_")
            && !matches!(
                key.as_str(),
                "prompt_tokens_details"
                    | "completion_tokens_details"
                    | "input_tokens_details"
                    | "output_tokens_details"
                    | "cost"
                    | "cost_details"
                    | "is_byok"
            )
        {
            usage_obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    crate::urp::usage::write_modality(
        &mut usage_value["prompt_tokens_details"],
        &input_details.modality_breakdown,
    );
    crate::urp::usage::write_modality(
        &mut usage_value["completion_tokens_details"],
        &output_details.modality_breakdown,
    );
    usage_value
}

fn encode_chat_content_part(part: &Part) -> Option<Value> {
    match part {
        Part::Text {
            content,
            extra_body,
            ..
        } => {
            let mut block = json!({ "type": "text", "text": content });
            if let Some(obj) = block.as_object_mut() {
                merge_chat_wire_extra(obj, extra_body);
            }
            Some(block)
        }
        Part::Image {
            metadata,
            source,
            extra_body,
        } => {
            let mut image = match source {
                ImageSource::Url { url, detail } => {
                    json!({ "type": "image_url", "image_url": { "url": url, "detail": detail } })
                }
                ImageSource::Base64 { media_type, data } => json!({
                    "type": "image_url",
                    "image_url": { "url": format!("data:{};base64,{}", media_type, data) }
                }),
                ImageSource::FileId { .. } => return None,
            };
            if let Some(obj) = image.as_object_mut() {
                merge_chat_wire_extra(obj, extra_body);
                for key in ["detail", "filename", "media_type", "source"] {
                    obj.remove(key);
                }
                let mut image_url = Map::new();
                let detail = match source {
                    ImageSource::Url { url, detail } => {
                        image_url.insert("url".into(), json!(url));
                        detail.as_ref()
                    }
                    ImageSource::Base64 { media_type, data } => {
                        image_url.insert(
                            "url".into(),
                            json!(format!("data:{media_type};base64,{data}")),
                        );
                        metadata.detail.as_ref()
                    }
                    ImageSource::FileId { .. } => return None,
                };
                if let Some(detail) = detail {
                    image_url.insert("detail".into(), json!(detail));
                }
                obj.insert("image_url".into(), Value::Object(image_url));
                obj.insert("type".into(), json!("image_url"));
            }
            Some(image)
        }
        Part::File {
            metadata,
            source,
            extra_body,
        } => encode_chat_file_part(source, metadata, extra_body),
        Part::Audio {
            source, extra_body, ..
        } => encode_chat_audio_part(source, extra_body),
        Part::ProviderItem {
            id,
            item_type,
            origin_protocol,
            body,
            extra_body,
        } => {
            encode_chat_provider_part(*origin_protocol, id.as_deref(), item_type, body, extra_body)
        }
        _ => None,
    }
}

fn encode_chat_file_part(
    source: &FileSource,
    metadata: &crate::urp::MediaMetadata,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let file = match source {
        FileSource::FileId { file_id }
            if crate::urp::media::resource_matches(metadata, ProviderProtocol::ChatCompletion) =>
        {
            json!({ "file_id": file_id })
        }
        FileSource::Base64 { media_type, data } => {
            let mut file = json!({ "file_data": format!("data:{media_type};base64,{data}") });
            if let Some(filename) = &metadata.filename {
                file["filename"] = json!(filename);
            }
            file
        }
        FileSource::Url { .. }
        | FileSource::FileId { .. }
        | FileSource::Text { .. }
        | FileSource::Content { .. } => return None,
    };
    let mut file = file;
    if let Some(filename) = &metadata.filename {
        file["filename"] = json!(filename);
    }
    let mut block = Map::new();
    merge_chat_wire_extra(&mut block, extra_body);
    for key in [
        "filename",
        "detail",
        "media_type",
        "source",
        "file_url",
        "url",
        "file_id",
        "file_data",
    ] {
        block.remove(key);
    }
    block.insert("type".into(), json!("file"));
    block.insert("file".into(), file);
    Some(Value::Object(block))
}

fn encode_chat_audio_part(
    source: &AudioSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let AudioSource::Base64 { media_type, data } = source else {
        return None;
    };
    let format = match media_type.as_str() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => return None,
    };
    let mut block = json!({
        "type": "input_audio",
        "input_audio": { "data": data, "format": format }
    });
    merge_chat_wire_extra(block.as_object_mut()?, extra_body);
    Some(block)
}

fn encode_chat_provider_part(
    origin_protocol: ProviderProtocol,
    id: Option<&str>,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    if origin_protocol != ProviderProtocol::ChatCompletion {
        return None;
    }
    Some(chat_provider_item_wire_body(
        id, item_type, body, extra_body,
    ))
}

fn chat_provider_item_wire_body(
    id: Option<&str>,
    item_type: &str,
    body: &Value,
    extra_body: &HashMap<String, Value>,
) -> Value {
    let mut part = sanitize_provider_item_wire_body(body);
    if let Some(obj) = part.as_object_mut() {
        merge_chat_wire_extra(obj, extra_body);
        obj.remove("id");
        obj.remove("type");
        // Synthetic identities must not add fields absent from the native envelope.
        if body.get("id").is_some()
            && let Some(id) = id
        {
            obj.insert("id".into(), json!(id));
        }
        if body.get("type").is_some() && !item_type.is_empty() {
            obj.insert("type".into(), json!(item_type));
        }
    }
    part
}

fn finalize_chat_message_content(m: &mut Map<String, Value>, content_parts: Vec<Value>) {
    if !content_parts.is_empty() {
        let can_collapse_single_text = content_parts.len() == 1
            && content_parts[0].get("type").and_then(|v| v.as_str()) == Some("text")
            && content_parts[0]
                .as_object()
                .map(|obj| obj.keys().all(|k| k == "type" || k == "text"))
                .unwrap_or(false);

        if can_collapse_single_text {
            if let Some(text) = content_parts[0].get("text").and_then(|v| v.as_str()) {
                m.insert("content".to_string(), Value::String(text.to_string()));
            }
        } else {
            m.insert("content".to_string(), Value::Array(content_parts));
        }
    } else {
        m.insert("content".to_string(), Value::String(String::new()));
    }
}

fn finalize_chat_response_content(m: &mut Map<String, Value>, content_parts: Vec<Value>) {
    if content_parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) != Some("text"))
    {
        m.insert("content".to_string(), Value::Array(content_parts));
        return;
    }

    let content = content_parts
        .into_iter()
        .filter_map(|part| {
            (part.get("type").and_then(|v| v.as_str()) == Some("text"))
                .then(|| part.get("text").and_then(|v| v.as_str()))
                .flatten()
                .map(str::to_string)
        })
        .collect::<Vec<_>>()
        .concat();
    m.insert("content".to_string(), Value::String(content));
}

fn flush_pending_chat_message(
    pending: &mut Option<PendingChatMessage>,
    out: &mut Vec<Value>,
    dialect: ChatDialect,
) {
    let Some(pending_msg) = pending.take() else {
        return;
    };
    let mut reasoning_fields = Map::new();
    insert_reasoning_fields(
        &mut reasoning_fields,
        &pending_msg.reasoning_parts,
        ReasoningTarget::Request { dialect },
    );
    if pending_msg.content_parts.is_empty()
        && pending_msg.tool_calls.is_empty()
        && pending_msg.refusal.is_none()
        && reasoning_fields.is_empty()
        && pending_msg.legacy_function_call.is_none()
        && pending_msg.message_extra.is_empty()
    {
        return;
    }

    let mut m = Map::new();
    m.insert(
        "role".to_string(),
        Value::String(role_to_str(pending_msg.role).to_string()),
    );
    finalize_chat_message_content(&mut m, pending_msg.content_parts);
    if let Some(refusal) = pending_msg.refusal {
        m.insert("refusal".to_string(), Value::String(refusal));
    }
    if !pending_msg.tool_calls.is_empty() {
        m.insert(
            "tool_calls".to_string(),
            Value::Array(pending_msg.tool_calls),
        );
    }
    if let Some(function_call) = pending_msg.legacy_function_call {
        m.insert("function_call".to_string(), function_call);
    }
    m.extend(reasoning_fields);
    merge_chat_wire_extra(&mut m, &pending_msg.message_extra);
    out.push(Value::Object(m));
}

fn push_part_into_pending_chat_message(
    pending: &mut Option<PendingChatMessage>,
    out: &mut Vec<Value>,
    role: Role,
    extra_body: &HashMap<String, Value>,
    part: &Part,
    dialect: ChatDialect,
) {
    if let Part::ProviderItem {
        id,
        item_type,
        body,
        extra_body,
        origin_protocol: ProviderProtocol::ChatCompletion,
        ..
    } = part
    {
        if extra_body
            .get(crate::urp::CHAT_MESSAGE_ITEM_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true)
        {
            flush_pending_chat_message(pending, out, dialect);
            out.push(chat_provider_item_wire_body(
                id.as_deref(),
                item_type,
                body,
                extra_body,
            ));
            return;
        }
    }

    let entry = pending.get_or_insert_with(|| PendingChatMessage {
        role,
        content_parts: Vec::new(),
        tool_calls: Vec::new(),
        refusal: None,
        reasoning_parts: Vec::new(),
        legacy_function_call: None,
        message_extra: extra_body.clone(),
    });

    match part {
        Part::Audio {
            metadata,
            source: _,
            extra_body,
        } if extra_body
            .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true) =>
        {
            if let Some(id) = &metadata.reference_id {
                entry
                    .message_extra
                    .insert("audio".into(), json!({ "id": id }));
            }
        }
        Part::ProviderItem {
            id,
            item_type,
            origin_protocol: ProviderProtocol::ChatCompletion,
            body,
            extra_body,
            ..
        } if extra_body
            .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true) =>
        {
            entry.message_extra.insert(
                "audio".to_string(),
                chat_provider_item_wire_body(id.as_deref(), item_type, body, extra_body),
            );
        }
        Part::Text { citations, .. } => {
            if !citations.is_empty() {
                entry
                    .message_extra
                    .entry("annotations".into())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .expect("canonical annotations array")
                    .extend(crate::urp::citations::encode(
                        citations,
                        crate::urp::ProviderProtocol::ChatCompletion,
                        0,
                    ));
            }
            if let Some(content) = encode_chat_content_part(part) {
                entry.content_parts.push(content);
            }
        }
        Part::Image { .. } | Part::Audio { .. } | Part::File { .. } | Part::ProviderItem { .. } => {
            if let Some(content) = encode_chat_content_part(part) {
                entry.content_parts.push(content);
            }
        }
        Part::Refusal { content, .. } => {
            entry.refusal = Some(content.clone());
        }
        Part::Reasoning { .. } => {
            entry.reasoning_parts.push(part.clone());
        }
        Part::ToolCall {
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
            ..
        } => {
            if *tool_type == ToolCallType::Function
                && extra_body
                    .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                let mut function_call =
                    json!({ "name": name, "arguments": tool_call_arguments_for_wire(arguments) });
                if let Some(obj) = function_call.as_object_mut() {
                    merge_chat_wire_extra(obj, extra_body);
                }
                entry.legacy_function_call = Some(function_call);
            } else {
                entry
                    .tool_calls
                    .push(encode_chat_tool_call(*tool_type, call_id, name, arguments));
            }
        }
    }
}

/// PC0: the Chat Completions wire dialect of one upstream attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatDialect {
    OpenAi,
    DeepSeek,
    OpenRouter,
    Vercel,
    Generic,
}

impl ChatDialect {
    pub fn resolve(base_url: &str, upstream_model: &str) -> Self {
        let host = reqwest::Url::parse(base_url.trim())
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        if host == "api.openai.com" || host.ends_with(".openai.azure.com") {
            Self::OpenAi
        } else if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
            Self::OpenRouter
        } else if host == "ai-gateway.vercel.sh" {
            Self::Vercel
        } else if host == "api.deepseek.com"
            || upstream_model.to_ascii_lowercase().contains("deepseek")
        {
            Self::DeepSeek
        } else {
            Self::Generic
        }
    }

    fn reasoning_details(self) -> bool {
        matches!(self, Self::OpenRouter | Self::Vercel | Self::Generic)
    }
}

/// A request the Chat encoder refuses before dispatch; `code` is the HTTP 400 error code.
#[derive(Debug, Clone)]
pub struct ChatEncodeError {
    pub code: &'static str,
    pub message: String,
}

pub fn encode_request(req: &UrpRequest, upstream_model: &str, dialect: ChatDialect) -> Value {
    encode_request_checked(req, upstream_model, dialect)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error.message))
}

pub fn encode_request_checked(
    req: &UrpRequest,
    upstream_model: &str,
    dialect: ChatDialect,
) -> Result<Value, ChatEncodeError> {
    let mut prepared = crate::urp::media::prepare_request(req, ProviderProtocol::ChatCompletion)
        .map_err(|message| ChatEncodeError {
            code: "unsupported_media",
            message,
        })?;
    crate::urp::cache_breakpoint::materialize_wire_markers(
        &mut prepared,
        crate::urp::cache_breakpoint::CacheWireTarget::Chat(dialect),
        upstream_model,
    );
    encode_request_prepared(&prepared, upstream_model, dialect)
}

fn encode_request_prepared(
    req: &UrpRequest,
    upstream_model: &str,
    dialect: ChatDialect,
) -> Result<Value, ChatEncodeError> {
    // PC2.11: an empty assistant text carries nothing and must not split the assistant turn.
    let input = req
        .input
        .iter()
        .filter(|node| {
            !matches!(node, Node::Text {
                role: OrdinaryRole::Assistant,
                content,
                citations,
                extra_body,
                ..
            } if content.is_empty()
                && citations.is_empty()
                && extra_body.keys().all(|key| key.starts_with("_monoize_")))
        })
        .cloned()
        .collect::<Vec<_>>();
    let request_items = nodes_to_items(&input);
    let mut body = json!({
        "model": upstream_model,
        "messages": encode_messages(&request_items, dialect),
    });

    let obj = body.as_object_mut().expect("chat request object");
    if let Some(stream) = req.stream {
        obj.insert("stream".to_string(), Value::Bool(stream));
    }
    if let Some(temp) = req.temperature {
        obj.insert("temperature".to_string(), Value::from(temp));
    }
    if let Some(top_p) = req.top_p {
        obj.insert("top_p".to_string(), Value::from(top_p));
    }
    if let Some(max) = req.max_output_tokens {
        let client_field = req
            .extra_body
            .get(crate::urp::CHAT_MAX_TOKENS_FIELD_EXTRA_KEY)
            .and_then(Value::as_str)
            .filter(|field| matches!(*field, "max_tokens" | "max_completion_tokens"));
        let key = match (dialect, client_field) {
            (ChatDialect::DeepSeek, _) => "max_tokens",
            (_, Some(field)) => field,
            (ChatDialect::OpenRouter | ChatDialect::Vercel, None) => "max_tokens",
            (ChatDialect::OpenAi | ChatDialect::Generic, None) => "max_completion_tokens",
        };
        obj.insert(key.to_string(), Value::from(max));
    }
    if let Some(reasoning) = &req.reasoning {
        encode_reasoning_controls(obj, reasoning, dialect);
    }
    if let Some(tools) = &req.tools {
        let (modern_tools, dropped_tool_names) = encode_tools(tools, dialect)?;
        let legacy_functions = encode_legacy_functions(tools);
        if !modern_tools.is_empty() {
            obj.insert("tools".to_string(), Value::Array(modern_tools));
        }
        if !legacy_functions.is_empty() {
            obj.insert("functions".to_string(), Value::Array(legacy_functions));
        }
        let has_tools = obj.contains_key("tools") || obj.contains_key("functions");
        if let Some(tc) = &req.tool_choice
            && has_tools
        {
            if let Some(raw_legacy_choice) =
                req.extra_body.get(CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY)
            {
                if let Some(choice) = encode_legacy_function_choice(tc, raw_legacy_choice) {
                    obj.insert("function_call".to_string(), choice);
                }
            } else if tool_choice_names_any(tc, &dropped_tool_names) {
                obj.insert("tool_choice".to_string(), json!("auto"));
            } else {
                obj.insert("tool_choice".to_string(), tool_choice_to_chat_value(tc));
            }
        }
        if let Some(parallel) = req.parallel_tool_calls
            && has_tools
        {
            obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
        }
    } else {
        if let Some(tc) = &req.tool_choice {
            if let Some(raw_legacy_choice) =
                req.extra_body.get(CHAT_LEGACY_FUNCTION_CHOICE_EXTRA_KEY)
            {
                if let Some(choice) = encode_legacy_function_choice(tc, raw_legacy_choice) {
                    obj.insert("function_call".to_string(), choice);
                }
            } else {
                obj.insert("tool_choice".to_string(), tool_choice_to_chat_value(tc));
            }
        }
        if let Some(parallel) = req.parallel_tool_calls {
            obj.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
        }
    }
    if let Some(stop) = &req.stop {
        let value = match stop {
            StopControl::Single(stop) => Value::String(stop.clone()),
            StopControl::Multiple(stops) => Value::Array(
                stops
                    .iter()
                    .map(|stop| Value::String(stop.clone()))
                    .collect(),
            ),
        };
        obj.insert("stop".to_string(), value);
    }
    if let Some(verbosity) = &req.verbosity {
        obj.insert("verbosity".to_string(), Value::String(verbosity.clone()));
    }
    if let Some(format) = &req.response_format {
        obj.insert(
            "response_format".to_string(),
            encode_response_format(format),
        );
    }
    if let Some(user) = &req.user {
        obj.insert("user".to_string(), Value::String(user.clone()));
    }

    merge_chat_wire_extra(obj, &req.extra_body);
    if dialect != ChatDialect::Vercel {
        obj.remove("providerOptions");
    }

    if req.stream == Some(true) {
        match obj.get_mut("stream_options") {
            Some(Value::Object(so)) => {
                so.insert("include_usage".to_string(), Value::Bool(true));
            }
            Some(_) => {
                obj.insert("stream_options".to_string(), json!({"include_usage": true}));
            }
            None => {
                obj.insert("stream_options".to_string(), json!({"include_usage": true}));
            }
        }
    }

    crate::urp::logprobs::encode_request(
        &mut body,
        &req.logprobs,
        crate::urp::ProviderProtocol::ChatCompletion,
    );
    crate::urp::sampling::encode_request(
        &mut body,
        &req.sampling,
        crate::urp::ProviderProtocol::ChatCompletion,
    );
    Ok(body)
}

/// RC4 for Chat targets, by dialect.
fn encode_reasoning_controls(
    obj: &mut Map<String, Value>,
    reasoning: &crate::urp::ReasoningConfig,
    dialect: ChatDialect,
) {
    let native_reasoning = reasoning
        .extra_body
        .get(CHAT_REASONING_CONFIG_EXTRA_KEY)
        .and_then(Value::as_object)
        .filter(|_| dialect.reasoning_details());
    let native_thinking = reasoning
        .extra_body
        .get(CHAT_THINKING_CONFIG_EXTRA_KEY)
        .and_then(Value::as_object);
    let effort = if reasoning.disabled() {
        Some("none")
    } else {
        reasoning.effort.as_deref()
    }
    .map(chat_wire_effort);
    // A native `thinking` object owns mode and budget.
    let budget = reasoning.budget_tokens.filter(|_| native_thinking.is_none());
    let mode = reasoning
        .mode
        .as_deref()
        .filter(|_| native_thinking.is_none());

    if let Some(native) = native_reasoning {
        let mut controls = native.clone();
        controls.retain(|key, _| {
            !matches!(
                key.as_str(),
                "effort" | "summary" | "max_tokens" | "enabled"
            )
        });
        if let Some(effort) = effort {
            controls.insert("effort".into(), json!(effort));
        } else if let Some(budget) = budget {
            controls.insert("max_tokens".into(), json!(budget));
        }
        if let Some(summary) = &reasoning.summary {
            controls.insert("summary".into(), json!(summary));
        }
        if let Some(mode) = mode {
            controls.insert("enabled".into(), json!(mode != "disabled"));
        }
        obj.insert("reasoning".into(), Value::Object(controls));
    } else if let Some(effort) = effort {
        if dialect == ChatDialect::DeepSeek && effort == "none" {
            // DeepSeek toggles thinking only via `thinking.type`; `reasoning_effort` takes
            // low/high/max.
            if native_thinking.is_none() {
                obj.insert("thinking".into(), json!({ "type": "disabled" }));
            }
        } else {
            obj.insert("reasoning_effort".into(), json!(effort));
        }
    } else if matches!(dialect, ChatDialect::OpenRouter | ChatDialect::Vercel) {
        if let Some(budget) = budget {
            obj.insert("reasoning".into(), json!({ "max_tokens": budget }));
        } else if mode.is_some_and(|mode| mode != "disabled") {
            obj.insert("reasoning".into(), json!({ "enabled": true }));
        }
    }

    // OpenAI Chat defines no `thinking` parameter.
    if let Some(native) = native_thinking.filter(|_| dialect != ChatDialect::OpenAi) {
        let mut thinking = native.clone();
        thinking.retain(|key, _| !matches!(key.as_str(), "type" | "budget_tokens" | "display"));
        if reasoning.disabled() {
            thinking.insert("type".into(), json!("disabled"));
        } else if let Some(mode) = &reasoning.mode {
            thinking.insert("type".into(), json!(mode));
        }
        if let Some(budget) = reasoning.budget_tokens {
            thinking.insert("budget_tokens".into(), json!(budget));
        }
        if let Some(display) = &reasoning.display {
            thinking.insert("display".into(), json!(display));
        }
        obj.insert("thinking".into(), Value::Object(thinking));
    }
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model)
        .unwrap_or_else(|error| crate::urp::media::error_body(&error))
}

pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    if let Some(body) = resp
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.failure_body(false))
    {
        return Ok(body);
    }
    validate_response_nodes(&resp.output)?;
    Ok(encode_response_validated(resp, logical_model))
}

pub(crate) fn validate_response_nodes(nodes: &[Node]) -> Result<(), String> {
    for node in nodes {
        match node {
            Node::Image { .. } | Node::File { .. } => {
                return Err(
                    "Chat Completions responses cannot represent ordinary image or file output"
                        .into(),
                );
            }
            Node::Audio {
                role: OrdinaryRole::Assistant,
                source,
                extra_body,
                ..
            } if extra_body
                .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true)
                && matches!(source, AudioSource::Base64 { .. }) => {}
            Node::Audio { .. } => {
                return Err(
                    "Chat Completions responses require native message.audio for audio output"
                        .into(),
                );
            }
            Node::ToolResult { content, .. }
                if content.iter().any(|part| {
                    matches!(
                        part,
                        ToolResultContent::Image { .. } | ToolResultContent::File { .. }
                    )
                }) =>
            {
                return Err("Chat Completions responses cannot represent tool-result media".into());
            }
            Node::ProviderItem { item_type, .. }
                if matches!(
                    item_type.as_str(),
                    "input_image"
                        | "output_image"
                        | "image_url"
                        | "input_file"
                        | "output_file"
                        | "file"
                        | "input_audio"
                ) =>
            {
                return Err("Native response content cannot contain input-only media items".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn encode_response_validated(resp: &UrpResponse, logical_model: &str) -> Value {
    let projected = crate::urp::tool_signature::project_response(resp);
    let resp = &projected;
    let message = encode_assistant_chat_message_from_nodes(&resp.output);
    let has_legacy_function_call = resp.output.iter().any(|node| {
        matches!(
            node,
            Node::ToolCall {
                tool_type: ToolCallType::Function,
                extra_body,
                ..
            } if extra_body
                .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true)
        )
    });

    let native_finish_reason = resp
        .extra_body
        .get(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY)
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    let finish_reason = match resp.finish_reason {
        Some(FinishReason::Other) => native_finish_reason.unwrap_or("stop"),
        Some(FinishReason::ToolCalls) if has_legacy_function_call => "function_call",
        Some(reason) => finish_reason_to_chat(reason),
        None => {
            if resp
                .output
                .iter()
                .any(|node| matches!(node, Node::ToolCall { .. }))
            {
                if has_legacy_function_call {
                    "function_call"
                } else {
                    "tool_calls"
                }
            } else {
                "stop"
            }
        }
    };

    let mut result = json!({
        "id": chat_completion_id(&resp.id),
        "object": "chat.completion",
        "created": resp
            .created_at
            .unwrap_or_else(|| chrono::Utc::now().timestamp()),
        "model": logical_model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
    });

    let mut content_scores = Vec::new();
    let mut refusal_scores = Vec::new();
    for node in &resp.output {
        match node {
            Node::Text {
                content, logprobs, ..
            } => {
                if let Some(scores) = crate::urp::logprobs::valid(logprobs, content) {
                    content_scores.extend_from_slice(scores);
                }
            }
            Node::Refusal {
                content, logprobs, ..
            } => {
                if let Some(scores) = crate::urp::logprobs::valid(logprobs, content) {
                    refusal_scores.extend_from_slice(scores);
                }
            }
            _ => {}
        }
    }
    let message = &result["choices"][0]["message"];
    let content_scores = Some(content_scores);
    let refusal_scores = Some(refusal_scores);
    let content_scores = message
        .get("content")
        .and_then(Value::as_str)
        .and_then(|text| crate::urp::logprobs::valid(&content_scores, text));
    let refusal_scores = message
        .get("refusal")
        .and_then(Value::as_str)
        .and_then(|text| crate::urp::logprobs::valid(&refusal_scores, text));
    if content_scores.is_some_and(|v| !v.is_empty())
        || refusal_scores.is_some_and(|v| !v.is_empty())
    {
        result["choices"][0]["logprobs"] = json!({
            "content": content_scores.map(crate::urp::logprobs::encode_openai),
            "refusal": refusal_scores.map(crate::urp::logprobs::encode_openai),
        });
    }
    if let Some(usage) = &resp.usage {
        result["usage"] = encode_chat_usage(usage);
    }

    if let Some(choice_extra) = resp
        .extra_body
        .get(CHAT_CHOICE_EXTRA_BODY_KEY)
        .and_then(Value::as_object)
        && let Some(choice) = result
            .get_mut("choices")
            .and_then(Value::as_array_mut)
            .and_then(|choices| choices.first_mut())
            .and_then(Value::as_object_mut)
    {
        for (key, value) in choice_extra {
            if !key.starts_with("_monoize_") && key != "logprobs" {
                choice.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }

    let obj = result.as_object_mut().expect("chat response object");
    let mut response_extra = resp.extra_body.clone();
    response_extra.remove(CHAT_CHOICE_EXTRA_BODY_KEY);
    response_extra.remove(CHAT_NATIVE_FINISH_REASON_EXTRA_KEY);
    merge_chat_wire_extra(obj, &response_extra);
    result
}

pub(crate) fn encode_assistant_chat_message_from_nodes(nodes: &[Node]) -> Map<String, Value> {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));

    let mut content_parts = Vec::new();
    let mut text_offset = 0u64;
    let mut text_phases: Vec<Option<&String>> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut refusal: Option<String> = None;
    let mut reasoning_parts = Vec::new();
    let mut legacy_function_call: Option<Value> = None;
    let mut message_extra = HashMap::new();

    for node in nodes {
        match node {
            Node::NextDownstreamEnvelopeExtra { extra_body } => {
                merge_extra_preserving_existing(
                    &mut message_extra,
                    extra_body
                        .iter()
                        .filter(|(key, _)| !key.starts_with("_monoize_"))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                );
            }
            Node::Text {
                role: OrdinaryRole::Assistant,
                content,
                citations,
                phase,
                extra_body,
                ..
            } => {
                if !citations.is_empty() {
                    message
                        .entry("annotations".to_string())
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .unwrap()
                        .extend(crate::urp::citations::encode(
                            citations,
                            crate::urp::ProviderProtocol::ChatCompletion,
                            text_offset,
                        ));
                }
                let mut block = json!({ "type": "text", "text": content });
                if let Some(obj) = block.as_object_mut() {
                    merge_chat_wire_extra(obj, extra_body);
                }
                text_phases.push(phase.as_ref());
                text_offset += content.chars().count() as u64;
                content_parts.push(block);
            }
            Node::Audio {
                metadata,
                source,
                extra_body,
                ..
            } if extra_body
                .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true) =>
            {
                if let Some(audio) = encode_chat_generated_audio(metadata, source, extra_body) {
                    message_extra.insert("audio".into(), audio);
                }
            }
            Node::Refusal { content, .. } => {
                refusal.get_or_insert_with(String::new).push_str(content);
            }
            Node::Reasoning {
                metadata,
                id,
                content,
                encrypted,
                summary,
                source,
                extra_body,
            } => {
                reasoning_parts.push(Part::Reasoning {
                    metadata: metadata.clone(),
                    id: id.clone(),
                    content: content.clone(),
                    encrypted: encrypted.clone(),
                    summary: summary.clone(),
                    source: source.clone(),
                    extra_body: extra_body.clone(),
                });
            }
            Node::ToolCall {
                tool_type,
                call_id,
                name,
                arguments,
                extra_body,
                ..
            } => {
                if *tool_type == ToolCallType::Function
                    && extra_body
                        .get(CHAT_LEGACY_FUNCTION_CALL_EXTRA_KEY)
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    let mut function_call = json!({ "name": name, "arguments": tool_call_arguments_for_wire(arguments) });
                    if let Some(obj) = function_call.as_object_mut() {
                        merge_chat_wire_extra(obj, extra_body);
                    }
                    legacy_function_call = Some(function_call);
                } else {
                    tool_calls.push(encode_chat_tool_call(*tool_type, call_id, name, arguments));
                }
            }
            Node::ProviderItem {
                id,
                item_type,
                role: OrdinaryRole::Assistant,
                origin_protocol: ProviderProtocol::ChatCompletion,
                body,
                extra_body,
                ..
            } if extra_body
                .get(CHAT_MESSAGE_AUDIO_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true) =>
            {
                message_extra.insert(
                    "audio".to_string(),
                    chat_provider_item_wire_body(id.as_deref(), item_type, body, extra_body),
                );
            }
            Node::ProviderItem {
                id,
                item_type,
                role: OrdinaryRole::Assistant,
                origin_protocol,
                body,
                extra_body,
                ..
            } => {
                if let Some(part) = encode_chat_provider_part(
                    *origin_protocol,
                    id.as_deref(),
                    item_type,
                    body,
                    extra_body,
                ) {
                    content_parts.push(part);
                }
            }
            _ => {}
        }
    }

    // DC4a: `phase` describes the whole message only when every text node agrees.
    if let Some(Some(first)) = text_phases.first()
        && text_phases.iter().all(|phase| *phase == Some(*first))
    {
        message_extra
            .entry("phase".to_string())
            .or_insert_with(|| Value::String((*first).clone()));
    }
    let had_content_parts = !content_parts.is_empty();
    finalize_chat_response_content(&mut message, content_parts);
    if let Some(refusal) = refusal {
        message.insert("refusal".to_string(), Value::String(refusal));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if let Some(function_call) = legacy_function_call {
        message.insert("function_call".to_string(), function_call);
    }
    insert_reasoning_fields(&mut message, &reasoning_parts, ReasoningTarget::Response);
    merge_chat_wire_extra(&mut message, &message_extra);
    if !had_content_parts
        && (message.contains_key("audio")
            || message.contains_key("function_call")
            || message.contains_key("tool_calls")
            || message.contains_key("refusal"))
    {
        message.insert("content".to_string(), Value::Null);
    }
    message
}

fn merge_extra_preserving_existing(dst: &mut HashMap<String, Value>, src: HashMap<String, Value>) {
    for (k, v) in src {
        dst.entry(k).or_insert(v);
    }
}

fn merge_chat_wire_extra(
    wire_object: &mut Map<String, Value>,
    extra_body: &HashMap<String, Value>,
) {
    for (key, value) in extra_body {
        if !key.starts_with("_monoize_") && !wire_object.contains_key(key) {
            wire_object.insert(key.clone(), value.clone());
        }
    }
}

fn encode_messages(messages: &[Item], dialect: ChatDialect) -> Vec<Value> {
    let mut out = Vec::new();
    for item in messages {
        match item {
            Item::ToolResult {
                id,
                call_id,
                name,
                content,
                is_error,
                extra_body,
                ..
            } => {
                let content = encode_chat_tool_result_content(content, *is_error);
                let mut m = Map::new();
                if extra_body.contains_key(CHAT_LEGACY_FUNCTION_RESULT_EXTRA_KEY) {
                    m.insert("role".to_string(), Value::String("function".to_string()));
                    m.insert("content".to_string(), content);
                } else {
                    m.insert("role".to_string(), Value::String("tool".to_string()));
                    m.insert("content".to_string(), content);
                    m.insert("tool_call_id".to_string(), Value::String(call_id.clone()));
                }
                merge_chat_wire_extra(&mut m, extra_body);
                m.remove("id");
                m.remove("name");
                if let Some(id) = id {
                    m.insert("id".to_string(), Value::String(id.clone()));
                }
                if let Some(name) = name {
                    m.insert("name".to_string(), Value::String(name.clone()));
                }
                out.push(Value::Object(m));
            }
            Item::Message {
                id: _,
                role,
                parts,
                extra_body,
            } => {
                if *role == Role::Tool {
                    let mut m = Map::new();
                    m.insert("role".to_string(), Value::String("tool".to_string()));
                    m.insert("content".to_string(), Value::String(text_parts(parts)));
                    merge_chat_wire_extra(&mut m, extra_body);
                    out.push(Value::Object(m));
                    continue;
                }

                let mut pending: Option<PendingChatMessage> = None;
                for part in parts {
                    push_part_into_pending_chat_message(
                        &mut pending,
                        &mut out,
                        *role,
                        extra_body,
                        part,
                        dialect,
                    );
                }
                flush_pending_chat_message(&mut pending, &mut out, dialect);
            }
        }
    }
    out
}

/// PC2.10: Chat tool messages accept a string or an array of text parts and have no error flag.
fn encode_chat_tool_result_content(content: &[ToolResultContent], is_error: bool) -> Value {
    let mut entries = content
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text {
                text, extra_body, ..
            } => Some((text.clone(), extra_body)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let no_extra = HashMap::new();
    if is_error {
        match entries.first_mut() {
            Some((first, _)) => first.insert_str(0, "Error: "),
            None => entries.push(("Error".to_string(), &no_extra)),
        }
    }
    if entries.is_empty() {
        return Value::String(String::new());
    }
    let mut parts = entries
        .into_iter()
        .map(|(text, extra_body)| {
            let mut part = Map::from_iter([
                ("type".to_string(), json!("text")),
                ("text".to_string(), Value::String(text)),
            ]);
            merge_chat_wire_extra(&mut part, extra_body);
            part
        })
        .collect::<Vec<_>>();
    if parts.len() == 1 && parts[0].len() == 2 {
        return parts.remove(0).remove("text").unwrap_or_default();
    }
    Value::Array(parts.into_iter().map(Value::Object).collect())
}

pub(crate) fn is_web_search_builtin(tool_type: &str) -> bool {
    matches!(tool_type, "web_search" | "web_search_preview")
        || tool_type
            .strip_prefix("web_search_")
            .is_some_and(|date| date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
}

/// PC3. Returns the Chat tool descriptors and the names of degraded (dropped) tools.
fn encode_tools(
    tools: &[ToolDefinition],
    dialect: ChatDialect,
) -> Result<(Vec<Value>, Vec<String>), ChatEncodeError> {
    let mut out = Vec::new();
    let mut dropped = Vec::new();
    for tool in tools {
        if tool
            .extra_body
            .get(CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY)
            .and_then(Value::as_bool)
            == Some(true)
        {
            continue;
        }
        if tool.tool_type == "function" {
            if let Some(function) = &tool.function {
                let mut fn_obj = Map::new();
                fn_obj.insert("name".to_string(), Value::String(function.name.clone()));
                if let Some(desc) = &function.description {
                    fn_obj.insert("description".to_string(), Value::String(desc.clone()));
                }
                if let Some(parameters) = &function.parameters {
                    fn_obj.insert("parameters".to_string(), parameters.clone());
                }
                if let Some(strict) = function.strict {
                    fn_obj.insert("strict".to_string(), Value::Bool(strict));
                }
                merge_chat_wire_extra(&mut fn_obj, &function.extra_body);

                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String("function".to_string()));
                obj.insert("function".to_string(), Value::Object(fn_obj));
                merge_chat_wire_extra(&mut obj, &tool.extra_body);
                out.push(Value::Object(obj));
            }
        } else if tool.tool_type == "custom"
            && let Some(custom) = &tool.custom
        {
            let mut custom_obj = Map::new();
            custom_obj.insert("name".to_string(), Value::String(custom.name.clone()));
            if let Some(desc) = &custom.description {
                custom_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(format) = &custom.format {
                custom_obj.insert("format".to_string(), format.clone());
            }
            merge_chat_wire_extra(&mut custom_obj, &custom.extra_body);

            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("custom".to_string()));
            obj.insert("custom".to_string(), Value::Object(custom_obj));
            merge_chat_wire_extra(&mut obj, &tool.extra_body);
            out.push(Value::Object(obj));
        } else if tool.origin_protocol == Some(ProviderProtocol::ChatCompletion)
            && tool.tool_type.starts_with("openrouter:")
        {
            if !matches!(dialect, ChatDialect::OpenRouter | ChatDialect::Generic) {
                return Err(ChatEncodeError {
                    code: "unsupported_tool",
                    message: format!(
                        "tool type `{}` is not available on this Chat Completions upstream",
                        tool.tool_type
                    ),
                });
            }
            let mut obj = Map::new();
            obj.insert("type".to_string(), json!(tool.tool_type));
            if let Some(name) = &tool.name {
                obj.insert("name".to_string(), json!(name));
            }
            if let Some(description) = &tool.description {
                obj.insert("description".to_string(), json!(description));
            }
            if let Some(Value::Object(config)) = &tool.config {
                for (key, value) in config {
                    if !key.starts_with("_monoize_") {
                        obj.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
            merge_chat_wire_extra(&mut obj, &tool.extra_body);
            out.push(Value::Object(obj));
        } else if is_web_search_builtin(&tool.tool_type) {
            if dialect == ChatDialect::OpenRouter {
                out.push(json!({ "type": "openrouter:web_search" }));
            }
            dropped.push(tool.name.clone().unwrap_or_else(|| tool.tool_type.clone()));
            dropped.push(tool.tool_type.clone());
        } else {
            return Err(ChatEncodeError {
                code: "unsupported_tool",
                message: format!(
                    "tool type `{}` cannot be encoded for a Chat Completions upstream",
                    tool.tool_type
                ),
            });
        }
    }
    Ok((out, dropped))
}

/// True when a specific tool choice selects one of `names` (by `type` or nested/flat `name`).
fn tool_choice_names_any(choice: &ToolChoice, names: &[String]) -> bool {
    let ToolChoice::Specific(Value::Object(selector)) = choice else {
        return false;
    };
    let selected = [
        selector.get("type").and_then(Value::as_str),
        selector.get("name").and_then(Value::as_str),
        selector
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str),
    ];
    selected
        .into_iter()
        .flatten()
        .any(|selected| names.iter().any(|name| name == selected))
}

fn encode_legacy_functions(tools: &[ToolDefinition]) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if tool.tool_type != "function"
            || tool
                .extra_body
                .get(CHAT_LEGACY_FUNCTION_DEFINITION_EXTRA_KEY)
                .and_then(Value::as_bool)
                != Some(true)
        {
            continue;
        }
        let Some(function) = &tool.function else {
            continue;
        };

        let mut function_obj = Map::new();
        function_obj.insert("name".to_string(), Value::String(function.name.clone()));
        if let Some(description) = &function.description {
            function_obj.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(parameters) = &function.parameters {
            function_obj.insert("parameters".to_string(), parameters.clone());
        }
        if let Some(strict) = function.strict {
            function_obj.insert("strict".to_string(), Value::Bool(strict));
        }
        merge_chat_wire_extra(&mut function_obj, &function.extra_body);
        merge_chat_wire_extra(&mut function_obj, &tool.extra_body);
        out.push(Value::Object(function_obj));
    }
    out
}

fn encode_legacy_function_choice(choice: &ToolChoice, raw_choice: &Value) -> Option<Value> {
    match choice {
        ToolChoice::Mode(mode) => Some(Value::String(mode.clone())),
        ToolChoice::Specific(Value::Object(choice_obj)) => {
            let name = choice_obj
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .or_else(|| choice_obj.get("name"))
                .and_then(Value::as_str)?;
            let sanitized_raw = sanitize_provider_item_wire_body(raw_choice);
            let mut legacy_choice = sanitized_raw.as_object().cloned().unwrap_or_default();
            legacy_choice.remove("name");
            legacy_choice.insert("name".to_string(), Value::String(name.to_string()));
            Some(Value::Object(legacy_choice))
        }
        ToolChoice::Specific(_) => None,
    }
}

fn encode_response_format(format: &ResponseFormat) -> Value {
    match format {
        ResponseFormat::Text => json!({ "type": "text" }),
        ResponseFormat::JsonObject => json!({ "type": "json_object" }),
        ResponseFormat::JsonSchema { json_schema } => {
            let mut schema_obj = Map::new();
            schema_obj.insert("name".to_string(), Value::String(json_schema.name.clone()));
            schema_obj.insert("schema".to_string(), json_schema.schema.clone());
            if let Some(desc) = &json_schema.description {
                schema_obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(strict) = json_schema.strict {
                schema_obj.insert("strict".to_string(), Value::Bool(strict));
            }
            merge_chat_wire_extra(&mut schema_obj, &json_schema.extra_body);
            json!({
                "type": "json_schema",
                "json_schema": Value::Object(schema_obj),
            })
        }
    }
}

#[derive(Clone, Copy)]
enum ReasoningTarget {
    /// A downstream Chat response message (DC5).
    Response,
    /// An assistant-history message of an upstream Chat request (PC8a).
    Request { dialect: ChatDialect },
}

fn reasoning_surface(extra_body: &HashMap<String, Value>) -> Option<&str> {
    extra_body
        .get(CHAT_REASONING_SURFACE_EXTRA_KEY)
        .and_then(Value::as_str)
}

/// Plain text of a reasoning node: non-empty content, otherwise non-empty summary.
fn reasoning_plain_text<'a>(content: Option<&'a str>, summary: Option<&'a str>) -> Option<&'a str> {
    content
        .filter(|content| !content.is_empty())
        .or(summary.filter(|summary| !summary.is_empty()))
}

/// Assigns sequential `index` values to synthesized details that carry none (DC5).
pub(crate) fn index_reasoning_details(details: &mut [Map<String, Value>], first: u64) -> u64 {
    let mut next = first;
    for detail in details {
        if let Some(index) = detail.get("index").and_then(Value::as_u64) {
            next = next.max(index + 1);
        } else {
            detail.insert("index".into(), json!(next));
            next += 1;
        }
    }
    next
}

fn insert_reasoning_fields(message: &mut Map<String, Value>, parts: &[Part], target: ReasoningTarget) {
    let mut details = Vec::new();
    let mut reasoning_text = String::new();
    let mut reasoning_content = String::new();
    let mut opaque: Option<Value> = None;
    // Synthesized details take indexes above every native one (DC5).
    let mut next_index = parts
        .iter()
        .filter_map(|part| match part {
            Part::Reasoning { extra_body, .. } => extra_body
                .get(CHAT_REASONING_DETAIL_EXTRA_KEY)?
                .get("index")?
                .as_u64(),
            _ => None,
        })
        .max()
        .map_or(0, |index| index + 1);

    for part in parts {
        let Part::Reasoning {
            metadata,
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } = part
        else {
            continue;
        };
        let plain = reasoning_plain_text(content.as_deref(), summary.as_deref());
        if let ReasoningTarget::Request { dialect, .. } = target {
            match dialect {
                ChatDialect::OpenAi => continue,
                ChatDialect::DeepSeek => {
                    reasoning_content.push_str(plain.unwrap_or_default());
                    continue;
                }
                ChatDialect::OpenRouter | ChatDialect::Vercel | ChatDialect::Generic => {}
            }
        }
        // A scalar surface owns only the typed field it carried; other typed fields that a
        // transform set still render as details.
        let mut content = content.as_deref();
        let mut encrypted = encrypted.as_ref();
        match reasoning_surface(extra_body) {
            Some(CHAT_REASONING_SURFACE_REASONING_CONTENT) => {
                reasoning_content.push_str(content.take().unwrap_or_default());
            }
            Some(CHAT_REASONING_SURFACE_REASONING_OPAQUE) => {
                if let Some(value) = encrypted.take().filter(|value| !value.is_null()) {
                    opaque = Some(value.clone());
                }
            }
            _ => {}
        }
        let plain = reasoning_plain_text(content, summary.as_deref());
        if metadata.chat_content {
            reasoning_content.push_str(plain.unwrap_or_default());
        }
        let native = extra_body.contains_key(CHAT_REASONING_DETAIL_EXTRA_KEY);
        let mut node_details = crate::urp::reasoning::chat_node_details(
            metadata,
            id.as_deref(),
            content,
            summary.as_deref(),
            encrypted,
            source.as_deref(),
            extra_body,
        );
        if !native {
            next_index = index_reasoning_details(&mut node_details, next_index);
        }
        details.extend(node_details.into_iter().map(Value::Object));
        // RC4e: same-Chat details are authoritative in requests; no alias is synthesized.
        if matches!(target, ReasoningTarget::Response) || !native {
            reasoning_text.push_str(plain.unwrap_or_default());
        }
    }

    if !reasoning_text.is_empty() {
        message.insert("reasoning".to_string(), Value::String(reasoning_text));
    }
    if !reasoning_content.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_content),
        );
    }
    if let Some(opaque) = opaque {
        message.insert("reasoning_opaque".to_string(), opaque);
    }
    if !details.is_empty() {
        message.insert("reasoning_details".to_string(), Value::Array(details));
    }
}

fn chat_wire_effort(effort: &str) -> &str {
    if effort == "minimum" {
        "minimal"
    } else {
        effort
    }
}

pub(crate) fn finish_reason_to_chat(finish_reason: FinishReason) -> &'static str {
    match finish_reason {
        FinishReason::Stop => "stop",
        FinishReason::Length
        | FinishReason::ContextLimit
        | FinishReason::Paused
        | FinishReason::Compaction => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Other => "stop",
    }
}

pub(crate) fn encode_chat_generated_audio(
    metadata: &crate::urp::MediaMetadata,
    source: &AudioSource,
    extra_body: &HashMap<String, Value>,
) -> Option<Value> {
    let AudioSource::Base64 { data, .. } = source else {
        return None;
    };
    let mut audio = Map::new();
    for (key, value) in extra_body {
        if !key.starts_with("_monoize_")
            && !matches!(key.as_str(), "id" | "data" | "transcript" | "expires_at")
        {
            audio.insert(key.clone(), value.clone());
        }
    }
    audio.insert("data".into(), json!(data));
    if let Some(id) = &metadata.reference_id {
        audio.insert("id".into(), json!(id));
    }
    if let Some(transcript) = &metadata.transcript {
        audio.insert("transcript".into(), json!(transcript));
    }
    if let Some(expires_at) = metadata.expires_at {
        audio.insert("expires_at".into(), json!(expires_at));
    }
    Some(Value::Object(audio))
}

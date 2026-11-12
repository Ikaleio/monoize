//! Gemini `GenerateContentRequest` encoding (GEM-3, GEM-4, PG4).

use super::generation::generation_config;
use super::media::validate_media_nodes;
use super::model::GeminiModel;
use super::parts::{content_extra, encode_node_part, encode_tool_result};
use super::signatures::{resolve_bound_signatures, sign_current_turn_calls};
use super::tools::{encode_tool_config, encode_tools, validate_tools};
use crate::urp::decode::gemini::{CONTINUATION_ID_PREFIX, node_extra};
use crate::urp::encode::merge_extra;
use crate::urp::media::{error_body, prepare_request};
use crate::urp::{
    GEMINI_CONTENT_EXTRA_KEY, GEMINI_SYSTEM_EXTRA_KEY, ImageSource, Node, OrdinaryRole,
    ProviderProtocol, StopControl, UrpRequest, is_internal_key,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    encode_request_checked(req, upstream_model).unwrap_or_else(|message| error_body(&message))
}

pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    if req
        .logprobs
        .as_ref()
        .is_some_and(|config| config.enabled && config.top_k.is_some_and(|count| count > 20))
    {
        return Err("Gemini logprobs must be in the range 0 through 20.".into());
    }
    if matches!(&req.stop, Some(StopControl::Multiple(stops)) if stops.len() > 5) {
        return Err("Gemini accepts at most 5 stop sequences.".into());
    }
    validate_tools(req)?;
    let prepared = prepare_request(req, ProviderProtocol::Gemini)?;
    validate_media_nodes(&prepared.input)?;
    if prepared.input.iter().any(|node| {
        matches!(node, Node::Image { role, .. } | Node::Audio { role, .. } | Node::File { role, .. } | Node::ProviderItem { role, .. }
            if matches!(role, OrdinaryRole::System | OrdinaryRole::Developer))
    }) {
        return Err(
            "Gemini system instructions support text only; media cannot be discarded".into(),
        );
    }
    Ok(encode_prepared_request(&prepared, upstream_model))
}

fn encode_prepared_request(req: &UrpRequest, upstream_model: &str) -> Value {
    let model = GeminiModel::parse(upstream_model);
    let (nodes, continuation) = split_continuation(&req.input);
    let (contents, system) = encode_contents(nodes, &model);

    let mut obj = Map::new();
    obj.insert("contents".into(), Value::Array(contents));
    if let Some(system) = system {
        obj.insert("systemInstruction".into(), system);
    }
    let generation = generation_config(req, &model);
    if !generation.is_empty() {
        obj.insert("generationConfig".into(), Value::Object(generation));
    }
    if let Some(tools) = req
        .tools
        .as_deref()
        .map(encode_tools)
        .filter(|tools| !tools.is_empty())
    {
        obj.insert("tools".into(), Value::Array(tools));
    }
    let tool_config = encode_tool_config(req);
    if !tool_config.is_empty() {
        obj.insert("toolConfig".into(), Value::Object(tool_config));
    }
    if let Some(tier) = req
        .extra_body
        .get("service_tier")
        .and_then(Value::as_str)
        .and_then(gemini_service_tier)
    {
        obj.insert("serviceTier".into(), json!(tier));
    }
    if let Some(token) = continuation {
        obj.insert("continuationToken".into(), token.clone());
    }
    if let Some(identifier) = req
        .extra_body
        .get("safety_identifier")
        .and_then(Value::as_str)
        .filter(|identifier| valid_label_value(identifier))
    {
        let mut labels = req
            .extra_body
            .get("labels")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        labels
            .entry("safety_identifier")
            .or_insert_with(|| json!(identifier));
        obj.insert("labels".into(), Value::Object(labels));
    }
    for (key, value) in &req.extra_body {
        if !is_internal_key(key)
            && !obj.contains_key(key)
            && !matches!(
                key.as_str(),
                "generationConfig"
                    | "tools"
                    | "toolConfig"
                    | "contents"
                    | "systemInstruction"
                    | "service_tier"
                    | "safety_identifier"
            )
        {
            obj.insert(key.clone(), value.clone());
        }
    }
    if !upstream_model.is_empty() {
        obj.remove("model");
    }
    Value::Object(obj)
}

/// One Content under construction.
struct PendingContent {
    role: OrdinaryRole,
    parts: Vec<Value>,
    extra: HashMap<String, Value>,
}

/// Groups request nodes into Contents and the system instruction (GEM-3, PG4d, PG4e).
/// System and developer nodes join `systemInstruction`. A Content starts at a node that
/// carries native Content members, at a role change, and at a node with different native
/// Content members. Envelope controls neither split Content nor add Content members.
fn encode_contents(nodes: &[Node], model: &GeminiModel) -> (Vec<Value>, Option<Value>) {
    let mut call_names = HashMap::new();
    for node in nodes {
        if let Node::ToolCall { call_id, name, .. } = node {
            call_names.entry(call_id.as_str()).or_insert(name.as_str());
        }
    }
    let mut contents = Vec::new();
    let mut pending: Option<PendingContent> = None;
    let mut system_parts = Vec::new();
    let mut system_extra = HashMap::new();
    for node in nodes {
        let encoded = match node {
            Node::ToolResult { .. } => {
                encode_tool_result(node, &call_names).map(|part| (OrdinaryRole::User, part))
            }
            _ => encode_node_part(node),
        };
        let Some((role, mut part)) = encoded else {
            continue;
        };
        if model.media_resolution {
            add_media_resolution(node, &mut part);
        }
        let extra = content_extra(node);
        if node_extra(node).contains_key(GEMINI_CONTENT_EXTRA_KEY) {
            flush_content(&mut pending, &mut contents);
        }
        if matches!(role, OrdinaryRole::System | OrdinaryRole::Developer) {
            flush_content(&mut pending, &mut contents);
            if let Some(native) = node_extra(node)
                .get(GEMINI_SYSTEM_EXTRA_KEY)
                .and_then(Value::as_object)
            {
                system_extra.extend(
                    native
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            system_extra.extend(extra);
            system_parts.push(part);
            continue;
        }
        if pending.as_ref().is_some_and(|content| {
            content.role != role || (!extra.is_empty() && content.extra != extra)
        }) {
            flush_content(&mut pending, &mut contents);
        }
        pending
            .get_or_insert_with(|| PendingContent {
                role,
                parts: Vec::new(),
                extra,
            })
            .parts
            .push(part);
    }
    flush_content(&mut pending, &mut contents);
    if model.requires_call_signatures {
        sign_current_turn_calls(&mut contents);
    }
    let system = (!system_parts.is_empty()).then(|| {
        let mut system = Map::new();
        system.insert("parts".into(), Value::Array(system_parts));
        merge_extra(&mut system, &system_extra);
        Value::Object(system)
    });
    (contents, system)
}

fn flush_content(pending: &mut Option<PendingContent>, contents: &mut Vec<Value>) {
    let Some(mut content) = pending.take() else {
        return;
    };
    if content.parts.is_empty() {
        return;
    }
    resolve_bound_signatures(&mut content.parts, true);
    let mut obj = Map::new();
    obj.insert(
        "role".into(),
        json!(if content.role == OrdinaryRole::Assistant {
            "model"
        } else {
            "user"
        }),
    );
    obj.insert("parts".into(), Value::Array(content.parts));
    merge_extra(&mut obj, &content.extra);
    contents.push(Value::Object(obj));
}

/// PG5d2: resuming a `CONTINUATION` stop requires the original contents unchanged plus the
/// token. When the trailing assistant turn (after the last user or tool-result node) carries a
/// continuation carrier, that turn is the partial output being resumed: it is cut and its
/// token returned. A continuation carrier anywhere else is stale and encodes as nothing.
fn split_continuation(nodes: &[Node]) -> (&[Node], Option<&Value>) {
    let turn_start = nodes
        .iter()
        .rposition(|node| {
            matches!(node, Node::ToolResult { .. })
                || node
                    .role()
                    .is_some_and(|role| role != OrdinaryRole::Assistant)
        })
        .map_or(0, |index| index + 1);
    // Appended hops carry one token each; only the latest resumes.
    let token = nodes[turn_start..]
        .iter()
        .rev()
        .find_map(|node| match node {
            Node::Reasoning {
                id,
                metadata,
                encrypted: Some(token),
                ..
            } if id
                .as_deref()
                .or(metadata.item_id.as_deref())
                .is_some_and(|id| id.starts_with(CONTINUATION_ID_PREFIX)) =>
            {
                Some(token)
            }
            _ => None,
        });
    match token {
        Some(token) => (&nodes[..turn_start], Some(token)),
        None => (nodes, None),
    }
}

/// PG4j: maps a typed image `detail` to the Gemini 3+ per-Part media resolution.
fn add_media_resolution(node: &Node, part: &mut Value) {
    let Node::Image {
        source, metadata, ..
    } = node
    else {
        return;
    };
    let detail = metadata.detail.as_deref().or(match source {
        ImageSource::Url { detail, .. } => detail.as_deref(),
        _ => None,
    });
    let level = match detail {
        Some("low") => "MEDIA_RESOLUTION_LOW",
        Some("high") => "MEDIA_RESOLUTION_HIGH",
        Some("original") => "MEDIA_RESOLUTION_ULTRA_HIGH",
        _ => return,
    };
    if let Some(part) = part.as_object_mut() {
        part.entry("mediaResolution")
            .or_insert_with(|| json!({ "level": level }));
    }
}

/// Cloud label value rule from the GenerateContent `labels` reference.
fn valid_label_value(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 63
        && value
            .chars()
            .all(|c| c.is_lowercase() || c.is_numeric() || c == '_' || c == '-')
}

fn gemini_service_tier(tier: &str) -> Option<&'static str> {
    match tier {
        "default" | "standard" | "standard_only" => Some("standard"),
        "flex" => Some("flex"),
        "priority" => Some("priority"),
        _ => None,
    }
}

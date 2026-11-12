//! Gemini `GenerateContentResponse` encoding (GEM-5, GEM-6, GEM-7, PG5).

use super::media::validate_media_nodes;
use super::object_member;
use super::parts::{content_extra, encode_node_part};
use super::signatures::resolve_bound_signatures;
use super::usage::encode_usage_metadata;
use crate::urp::encode::merge_extra;
use crate::urp::media::{error_body, prepare_nodes};
use crate::urp::{
    FinishReason, GEMINI_CANDIDATE_EXTRA_KEY, Node, OrdinaryRole, ProviderProtocol, ResponseStatus,
    UrpResponse, citations, logprobs,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    encode_response_checked(resp, logical_model).unwrap_or_else(|message| error_body(&message))
}

pub fn encode_response_checked(resp: &UrpResponse, logical_model: &str) -> Result<Value, String> {
    if let Some(body) = resp
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.failure_body(false))
    {
        return Ok(body);
    }
    let output = prepare_response_nodes(&resp.output)?;
    Ok(encode_prepared_response(resp, output, logical_model))
}

/// The Parts of one streamed node. A single node has no adjacent Part to bind, so its signature
/// carriers stay standalone.
#[cfg(test)]
pub(crate) fn encode_response_node_parts(node: &Node) -> Result<Vec<Value>, String> {
    let mut parts: Vec<Value> = prepare_response_nodes(std::slice::from_ref(node))?
        .iter()
        .filter_map(encode_node_part)
        .map(|(_, part)| part)
        .filter(|part| !part.is_null())
        .collect();
    resolve_bound_signatures(&mut parts, false);
    Ok(parts)
}

/// Media-prepared response nodes. Every node Gemini cannot carry, and every non-assistant
/// node, fails the response.
fn prepare_response_nodes(nodes: &[Node]) -> Result<Vec<Node>, String> {
    let prepared = prepare_nodes(nodes, ProviderProtocol::Gemini)?;
    validate_media_nodes(&prepared)?;
    if prepared.iter().any(|node| {
        node.role()
            .is_some_and(|role| role != OrdinaryRole::Assistant)
    }) {
        return Err("Gemini response content must have the assistant role".into());
    }
    Ok(prepared)
}

fn encode_prepared_response(resp: &UrpResponse, output: Vec<Node>, logical_model: &str) -> Value {
    let mut parts = Vec::new();
    let mut emitted = Vec::new();
    let mut content_members = HashMap::new();
    for node in output {
        for (key, value) in content_extra(&node) {
            content_members.entry(key).or_insert(value);
        }
        if let Some((OrdinaryRole::Assistant, part)) = encode_node_part(&node)
            && !part.is_null()
        {
            parts.push(part);
            emitted.push(node);
        }
    }
    for removed in resolve_bound_signatures(&mut parts, false) {
        emitted.remove(removed);
    }

    let mut content = Map::new();
    content.insert("role".into(), json!("model"));
    content.insert("parts".into(), Value::Array(parts));
    merge_extra(&mut content, &content_members);
    let mut candidate = Map::new();
    candidate.insert("index".into(), json!(0));
    candidate.insert("content".into(), Value::Object(content));
    if let Some(reason) = response_finish_reason(resp) {
        candidate.insert(
            "finishReason".into(),
            json!(finish_reason_to_gemini(reason)),
        );
    }
    if let Some(metadata) = resp
        .extra_body
        .get(GEMINI_CANDIDATE_EXTRA_KEY)
        .and_then(Value::as_object)
    {
        for (key, value) in metadata {
            if !matches!(
                key.as_str(),
                "content" | "finishReason" | "logprobsResult" | "avgLogprobs"
            ) {
                candidate
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    encode_candidate_annotations(&mut candidate, &emitted);

    let mut body = Map::new();
    body.insert(
        "candidates".into(),
        Value::Array(vec![Value::Object(candidate)]),
    );
    if let Some(usage) = &resp.usage {
        body.insert("usageMetadata".into(), encode_usage_metadata(usage));
    }
    body.insert("modelVersion".into(), json!(logical_model));
    body.insert("responseId".into(), json!(resp.id));
    merge_extra(&mut body, &resp.extra_body);
    if is_prompt_block_response(resp) {
        body.insert("candidates".into(), json!([]));
    } else if let Some(feedback) = body
        .get_mut("promptFeedback")
        .and_then(Value::as_object_mut)
    {
        feedback.remove("blockReason");
    }
    Value::Object(body)
}

/// Citations, grounding, and token scores derived from the current typed nodes. Retained
/// native copies never restore a deleted citation or score (GEM-6).
fn encode_candidate_annotations(candidate: &mut Map<String, Value>, emitted: &[Node]) {
    let sources = citations::encode_gemini(emitted);
    if !sources.is_empty() {
        let metadata = object_member(candidate, "citationMetadata");
        metadata.insert("citationSources".into(), Value::Array(sources));
    } else if let Some(metadata) = candidate
        .get_mut("citationMetadata")
        .and_then(Value::as_object_mut)
    {
        metadata.remove("citationSources");
    }
    if let Some(metadata) = candidate
        .get_mut("groundingMetadata")
        .and_then(Value::as_object_mut)
    {
        if metadata.contains_key("groundingSupports") {
            metadata.remove("groundingChunks");
        }
        metadata.remove("groundingSupports");
    }
    if let Some(Value::Object(grounding)) = citations::encode_gemini_grounding(emitted) {
        object_member(candidate, "groundingMetadata").extend(grounding);
    }
    if let Some(scores) = logprobs::encode_gemini(emitted) {
        candidate.insert("logprobsResult".into(), scores);
        if let Some(average) = logprobs::gemini_summary(emitted) {
            candidate.insert("avgLogprobs".into(), json!(average));
        }
    }
}

/// A response that only reports a blocked prompt: Gemini sends it without candidates.
pub(crate) fn is_prompt_block_response(resp: &UrpResponse) -> bool {
    if response_finish_reason(resp) != Some(FinishReason::ContentFilter) {
        return false;
    }
    let Some(reason) = resp
        .extra_body
        .get("promptFeedback")
        .and_then(|value| value.get("blockReason"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    matches!(resp.output.as_slice(), [Node::Refusal { content, .. }] if *content == format!("Gemini blocked the prompt: {reason}"))
}

/// GEM-5: a typed outcome decides the native finish reason; queued and in-progress responses
/// have none.
fn response_finish_reason(response: &UrpResponse) -> Option<FinishReason> {
    if let Some(outcome) = &response.outcome {
        match outcome.status {
            ResponseStatus::Queued | ResponseStatus::InProgress => return None,
            ResponseStatus::Incomplete => {
                return Some(match outcome.incomplete_reason.as_deref() {
                    Some("max_output_tokens" | "context_length_exceeded") => FinishReason::Length,
                    Some("content_filter") => FinishReason::ContentFilter,
                    _ => match response.finish_reason {
                        Some(FinishReason::ContentFilter) => FinishReason::ContentFilter,
                        _ => FinishReason::Length,
                    },
                });
            }
            ResponseStatus::Completed => return Some(FinishReason::Stop),
            _ => {}
        }
    }
    response.finish_reason
}

fn finish_reason_to_gemini(finish_reason: FinishReason) -> &'static str {
    match finish_reason {
        FinishReason::Length | FinishReason::ContextLimit => "MAX_TOKENS",
        FinishReason::ToolCalls | FinishReason::Stop => "STOP",
        FinishReason::ContentFilter => "SAFETY",
        _ => "OTHER",
    }
}

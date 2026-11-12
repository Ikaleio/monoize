//! Gemini `GenerateContentResponse` decoding (GEM-5, PG5, PG5d) and the candidate rules the
//! stream decoder shares.

use super::carriers::{
    CONTINUATION_TOKEN_KEY, continuation_carrier, continuation_token, project_response_node,
    text_bytes,
};
use super::parts::{content_parts, decode_part};
use super::usage::parse_usage;
use crate::urp::decode::split_extra;
use crate::urp::outcome::ResponseError;
use crate::urp::{
    FinishReason, GEMINI_CANDIDATE_EXTRA_KEY, GEMINI_CONTENT_EXTRA_KEY, Node, OrdinaryRole,
    ResponseOutcome, UrpResponse, citations, logprobs,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "gemini response must be object".to_string())?;
    if let Some(error) = native_error(value) {
        return Err(error);
    }
    let candidate = selected_candidate(value);
    let blocked = prompt_block_reason(value);
    if candidate.is_none() && blocked.is_none() {
        return Err("missing candidates[0]".to_string());
    }

    let mut decoded = match candidate
        .and_then(|candidate| candidate.get("content"))
        .and_then(Value::as_object)
    {
        Some(content) => decode_candidate_content(content)?,
        None => Vec::new(),
    };
    if let Some(candidate) = candidate {
        citations::attach_gemini(candidate, &mut decoded);
        logprobs::attach_gemini(candidate, &mut decoded);
    }
    let mut output = Vec::with_capacity(decoded.len());
    let mut text_offset = 0;
    for node in decoded {
        let projected = project_response_node(node, text_offset);
        text_offset += text_bytes(&projected);
        output.extend(projected);
    }
    if let Some(token) = candidate.and_then(continuation_token) {
        output.push(continuation_carrier(token));
    }

    let mut finish_reason = candidate.and_then(native_finish_reason);
    if let Some(reason) = blocked {
        output.push(prompt_refusal(reason));
        finish_reason = Some(FinishReason::ContentFilter);
    } else if finish_reason == Some(FinishReason::Stop)
        && output
            .iter()
            .any(|node| matches!(node, Node::ToolCall { .. }))
    {
        finish_reason = Some(FinishReason::ToolCalls);
    }
    let usage = obj
        .get("usageMetadata")
        .and_then(Value::as_object)
        .map(parse_usage)
        .transpose()?;

    let mut extra_body = split_extra(
        obj,
        &[
            "candidates",
            "usageMetadata",
            "modelVersion",
            "responseId",
            "id",
            "model",
        ],
    );
    if let Some(metadata) = candidate
        .map(candidate_extra)
        .filter(|metadata| !metadata.is_empty())
    {
        extra_body.insert(
            GEMINI_CANDIDATE_EXTRA_KEY.into(),
            Value::Object(metadata.into_iter().collect()),
        );
    }
    Ok(UrpResponse {
        outcome: response_outcome(candidate, finish_reason),
        id: obj
            .get("responseId")
            .or_else(|| obj.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("resp_{}", uuid::Uuid::new_v4())),
        model: obj
            .get("modelVersion")
            .or_else(|| obj.get("model"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // GenerateContentResponse has no creation time; record the decode time (PG5d1).
        created_at: Some(chrono::Utc::now().timestamp()),
        output,
        finish_reason,
        usage,
        extra_body,
    })
}

/// Assistant nodes of one candidate Content before carrier projection. Unknown Content members
/// stay on the first node.
fn decode_candidate_content(content: &Map<String, Value>) -> Result<Vec<Node>, String> {
    let mut nodes = content
        .get("parts")
        .map(content_parts)
        .unwrap_or_default()
        .iter()
        .map(|part| decode_part(part, OrdinaryRole::Assistant))
        .collect::<Result<Vec<_>, _>>()?;
    let content_extra = split_extra(content, &["role", "parts"]);
    if let Some(first) = nodes.first_mut()
        && !content_extra.is_empty()
    {
        first.extra_body_mut().insert(
            GEMINI_CONTENT_EXTRA_KEY.into(),
            Value::Object(content_extra.into_iter().collect()),
        );
    }
    Ok(nodes)
}

/// GEM-5: candidate index zero when present, otherwise the first candidate.
pub(crate) fn selected_candidate(value: &Value) -> Option<&Map<String, Value>> {
    let candidates = value.get("candidates")?.as_array()?;
    candidates
        .iter()
        .find(|candidate| candidate.get("index").and_then(Value::as_u64) == Some(0))
        .or_else(|| candidates.first())?
        .as_object()
}

/// The message of a native error object, which fails decoding even beside candidates.
fn native_error(value: &Value) -> Option<String> {
    let error = value.get("error").filter(|error| !error.is_null())?;
    Some(
        error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Gemini API error: {error}")),
    )
}

pub(crate) fn prompt_block_reason(value: &Value) -> Option<&str> {
    value
        .get("promptFeedback")?
        .get("blockReason")?
        .as_str()
        .filter(|reason| !reason.is_empty() && *reason != "BLOCK_REASON_UNSPECIFIED")
}

pub(crate) fn prompt_refusal(reason: &str) -> Node {
    Node::Refusal {
        logprobs: None,
        id: None,
        content: format!("Gemini blocked the prompt: {reason}"),
        extra_body: HashMap::new(),
    }
}

/// The typed finish reason of a candidate; absent and unspecified reasons stay absent.
pub(crate) fn native_finish_reason(candidate: &Map<String, Value>) -> Option<FinishReason> {
    candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty() && *reason != "FINISH_REASON_UNSPECIFIED")
        .map(parse_finish_reason)
}

fn parse_finish_reason(reason: &str) -> FinishReason {
    match reason {
        // CONTINUATION stops at the per-request token cap; the answer is resumable, not failed.
        "MAX_TOKENS" | "CONTINUATION" => FinishReason::Length,
        "SAFETY"
        | "RECITATION"
        | "LANGUAGE"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION"
        | "ESCALATION"
        | "PUP_LIMITED_DISABLED" => FinishReason::ContentFilter,
        "STOP" => FinishReason::Stop,
        // OTHER, MALFORMED_FUNCTION_CALL, UNEXPECTED_TOOL_CALL, TOO_MANY_TOOL_CALLS,
        // MISSING_THOUGHT_SIGNATURE, MALFORMED_RESPONSE, IMAGE_OTHER, NO_IMAGE and unknown values.
        _ => FinishReason::Other,
    }
}

/// Typed outcome of a terminated candidate. `Other` fails with the native reason as error code;
/// a `CONTINUATION` stop records its token as incomplete detail.
pub(crate) fn response_outcome(
    candidate: Option<&Map<String, Value>>,
    finish: Option<FinishReason>,
) -> Option<ResponseOutcome> {
    let mut outcome = ResponseOutcome::from_finish(Some(finish?));
    if let Some(token) = candidate.and_then(continuation_token) {
        outcome
            .incomplete_extra
            .insert(CONTINUATION_TOKEN_KEY.into(), json!(token));
    }
    if finish == Some(FinishReason::Other) {
        outcome.error = Some(ResponseError {
            code: candidate
                .and_then(|candidate| candidate.get("finishReason"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            message: candidate
                .and_then(|candidate| candidate.get("finishMessage"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            extra_body: HashMap::new(),
        });
    }
    Some(outcome)
}

/// Candidate members that no typed field owns. Grounding metadata keeps only its presentation
/// members; citation metadata keeps only unknown members.
pub(crate) fn candidate_extra(candidate: &Map<String, Value>) -> HashMap<String, Value> {
    let mut extra = split_extra(
        candidate,
        &[
            "content",
            "finishReason",
            "citationMetadata",
            "logprobsResult",
            "avgLogprobs",
            "groundingMetadata",
        ],
    );
    if let Some(metadata) = citations::gemini_metadata_extra(candidate) {
        extra.insert("groundingMetadata".into(), metadata);
    }
    if let Some(metadata) = candidate.get("citationMetadata").and_then(Value::as_object) {
        let unknown = split_extra(metadata, &["citationSources"]);
        if !unknown.is_empty() {
            extra.insert(
                "citationMetadata".into(),
                Value::Object(unknown.into_iter().collect()),
            );
        }
    }
    extra
}

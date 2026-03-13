use super::StreamRuntimeMetrics;
use crate::urp;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) async fn mark_stream_ttfb_if_needed(
    started_at: Option<std::time::Instant>,
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) {
    let Some(started_at) = started_at else {
        return;
    };
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    if guard.ttfb_ms.is_none() {
        guard.ttfb_ms = Some(started_at.elapsed().as_millis() as u64);
    }
}

pub(crate) async fn record_stream_usage_if_present(
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    usage: Option<urp::Usage>,
) {
    let Some(usage) = usage else {
        return;
    };
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    let new_total = usage.total_tokens();
    let replace = match guard.usage.as_ref() {
        Some(existing) => {
            let existing_total = existing.total_tokens();
            new_total >= existing_total
        }
        None => true,
    };
    if replace {
        guard.usage = Some(usage);
    }
}

pub(crate) async fn latest_stream_usage_snapshot(
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) -> Option<urp::Usage> {
    let runtime_metrics = runtime_metrics.as_ref()?;
    let guard = runtime_metrics.lock().await;
    guard.usage.clone()
}

pub(crate) async fn record_stream_done_sentinel(
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) {
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    guard.terminal.saw_done_sentinel = true;
}

pub(crate) async fn record_stream_terminal_event(
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
    event: &str,
    finish_reason: Option<&str>,
) {
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    guard.terminal.terminal_event = Some(event.to_string());
    if let Some(reason) = finish_reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        guard.terminal.terminal_finish_reason = Some(reason.to_string());
    }
}

pub(crate) async fn record_stream_synthetic_terminal_emitted(
    runtime_metrics: &Option<Arc<Mutex<StreamRuntimeMetrics>>>,
) {
    let Some(runtime_metrics) = runtime_metrics.as_ref() else {
        return;
    };
    let mut guard = runtime_metrics.lock().await;
    guard.terminal.synthetic_terminal_emitted = true;
}

pub(crate) fn usage_to_chat_usage_json(usage: &urp::Usage) -> Value {
    let mut obj = json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens(),
        "completion_tokens_details": {
            "reasoning_tokens": usage.reasoning_tokens().unwrap_or(0),
            "accepted_prediction_tokens": usage.output_details.as_ref().map(|d| d.accepted_prediction_tokens).unwrap_or(0),
            "rejected_prediction_tokens": usage.output_details.as_ref().map(|d| d.rejected_prediction_tokens).unwrap_or(0)
        },
        "prompt_tokens_details": {
            "cached_tokens": usage.cached_tokens().unwrap_or(0),
            "cache_write_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_tokens).unwrap_or(0),
            "cache_creation_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_tokens).unwrap_or(0),
            "tool_prompt_tokens": usage.input_details.as_ref().map(|d| d.tool_prompt_tokens).unwrap_or(0)
        }
    });
    // Overwrite with full upstream detail objects (e.g. cache_write_tokens)
    if let Some(map) = obj.as_object_mut() {
        for (k, v) in &usage.extra_body {
            map.insert(k.clone(), v.clone());
        }
    }
    obj
}

pub(crate) fn usage_to_responses_usage_json(usage: &urp::Usage) -> Value {
    let mut obj = json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens(),
        "output_tokens_details": {
            "reasoning_tokens": usage.reasoning_tokens().unwrap_or(0),
            "accepted_prediction_tokens": usage.output_details.as_ref().map(|d| d.accepted_prediction_tokens).unwrap_or(0),
            "rejected_prediction_tokens": usage.output_details.as_ref().map(|d| d.rejected_prediction_tokens).unwrap_or(0)
        },
        "input_tokens_details": {
            "cached_tokens": usage.cached_tokens().unwrap_or(0),
            "cache_write_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_tokens).unwrap_or(0),
            "cache_creation_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_tokens).unwrap_or(0),
            "tool_prompt_tokens": usage.input_details.as_ref().map(|d| d.tool_prompt_tokens).unwrap_or(0)
        }
    });
    if let Some(map) = obj.as_object_mut() {
        for (k, v) in &usage.extra_body {
            map.insert(k.clone(), v.clone());
        }
    }
    obj
}

pub(crate) fn usage_to_messages_usage_json(usage: &urp::Usage) -> Value {
    let mut obj = json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_input_tokens": usage.cached_tokens().unwrap_or(0),
        "cache_creation_input_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_tokens).unwrap_or(0)
    });
    if let Some(map) = obj.as_object_mut() {
        for (k, v) in &usage.extra_body {
            map.insert(k.clone(), v.clone());
        }
    }
    obj
}

fn split_usage_extra(usage: &Map<String, Value>, known_keys: &[&str]) -> HashMap<String, Value> {
    usage
        .iter()
        .filter_map(|(k, v)| {
            if known_keys.contains(&k.as_str()) {
                None
            } else {
                Some((k.clone(), v.clone()))
            }
        })
        .collect()
}

fn parse_modality_breakdown_from_detail_object(
    detail: Option<&Map<String, Value>>,
) -> Option<urp::ModalityBreakdown> {
    let detail = detail?;
    let modality = detail
        .get("modality_breakdown")
        .and_then(|v| v.as_object())
        .unwrap_or(detail);
    let breakdown = urp::ModalityBreakdown {
        text_tokens: modality.get("text_tokens").and_then(|v| v.as_u64()),
        image_tokens: modality.get("image_tokens").and_then(|v| v.as_u64()),
        audio_tokens: modality.get("audio_tokens").and_then(|v| v.as_u64()),
        video_tokens: modality.get("video_tokens").and_then(|v| v.as_u64()),
        document_tokens: modality.get("document_tokens").and_then(|v| v.as_u64()),
    };
    if breakdown.text_tokens.is_some()
        || breakdown.image_tokens.is_some()
        || breakdown.audio_tokens.is_some()
        || breakdown.video_tokens.is_some()
        || breakdown.document_tokens.is_some()
    {
        Some(breakdown)
    } else {
        None
    }
}

fn make_input_details(
    standard_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    tool_prompt_tokens: u64,
    modality_breakdown: Option<urp::ModalityBreakdown>,
) -> Option<urp::InputDetails> {
    if standard_tokens > 0
        || cache_read_tokens > 0
        || cache_creation_tokens > 0
        || tool_prompt_tokens > 0
        || modality_breakdown.is_some()
    {
        Some(urp::InputDetails {
            standard_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            tool_prompt_tokens,
            modality_breakdown,
        })
    } else {
        None
    }
}

fn make_output_details(
    standard_tokens: u64,
    reasoning_tokens: u64,
    accepted_prediction_tokens: u64,
    rejected_prediction_tokens: u64,
    modality_breakdown: Option<urp::ModalityBreakdown>,
) -> Option<urp::OutputDetails> {
    if standard_tokens > 0
        || reasoning_tokens > 0
        || accepted_prediction_tokens > 0
        || rejected_prediction_tokens > 0
        || modality_breakdown.is_some()
    {
        Some(urp::OutputDetails {
            standard_tokens,
            reasoning_tokens,
            accepted_prediction_tokens,
            rejected_prediction_tokens,
            modality_breakdown,
        })
    } else {
        None
    }
}

pub(crate) fn parse_usage_from_chat_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj.get("usage")?.as_object()?;
    let input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_u64())?;
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|v| v.as_u64())?;
    let prompt_details = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"))
        .and_then(|v| v.as_object());
    let completion_details = usage
        .get("completion_tokens_details")
        .or_else(|| usage.get("output_tokens_details"))
        .and_then(|v| v.as_object());
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|v| v.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|v| v.get("cached_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|v| v.get("cache_write_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|v| v.get("cache_creation_tokens"))
        })
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|v| v.get("cache_write_tokens"))
        })
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|v| v.get("cache_creation_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tool_prompt_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|v| v.get("tool_prompt_tokens"))
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|v| v.get("tool_prompt_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(|v| v.get("reasoning_tokens"))
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|v| v.get("reasoning_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let accepted_prediction_tokens = usage
        .get("completion_tokens_details")
        .and_then(|v| v.get("accepted_prediction_tokens"))
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|v| v.get("accepted_prediction_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let rejected_prediction_tokens = usage
        .get("completion_tokens_details")
        .and_then(|v| v.get("rejected_prediction_tokens"))
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|v| v.get("rejected_prediction_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let extra_body = split_usage_extra(
        usage,
        &["prompt_tokens", "completion_tokens", "input_tokens", "output_tokens"],
    );
    Some(urp::Usage {
        input_tokens,
        output_tokens,
        input_details: make_input_details(
            0,
            cached_tokens,
            cache_creation_tokens,
            tool_prompt_tokens,
            parse_modality_breakdown_from_detail_object(prompt_details),
        ),
        output_details: make_output_details(
            0,
            reasoning_tokens,
            accepted_prediction_tokens,
            rejected_prediction_tokens,
            parse_modality_breakdown_from_detail_object(completion_details),
        ),
        extra_body,
    })
}

pub(crate) fn parse_usage_from_responses_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj
        .get("usage")
        .or_else(|| obj.get("response").and_then(|v| v.get("usage")))?;
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64())?;
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64())?;
    let input_details_obj = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"))
        .and_then(|v| v.as_object());
    let output_details_obj = usage
        .get("output_tokens_details")
        .or_else(|| usage.get("completion_tokens_details"))
        .and_then(|v| v.as_object());
    let cached_tokens = usage
        .get("input_tokens_details")
        .and_then(|v| v.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|v| v.get("cached_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|v| v.get("reasoning_tokens"))
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|v| v.get("reasoning_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("input_tokens_details")
        .and_then(|v| v.get("cache_creation_tokens"))
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|v| v.get("cache_write_tokens"))
        })
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|v| v.get("cache_creation_tokens"))
        })
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|v| v.get("cache_write_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tool_prompt_tokens = usage
        .get("input_tokens_details")
        .and_then(|v| v.get("tool_prompt_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|v| v.get("tool_prompt_tokens"))
        })
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let accepted_prediction_tokens = usage
        .get("output_tokens_details")
        .and_then(|v| v.get("accepted_prediction_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let rejected_prediction_tokens = usage
        .get("output_tokens_details")
        .and_then(|v| v.get("rejected_prediction_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let extra_body = split_usage_extra(
        usage.as_object()?,
        &["input_tokens", "output_tokens", "prompt_tokens", "completion_tokens"],
    );
    Some(urp::Usage {
        input_tokens,
        output_tokens,
        input_details: make_input_details(
            0,
            cached_tokens,
            cache_creation_tokens,
            tool_prompt_tokens,
            parse_modality_breakdown_from_detail_object(input_details_obj),
        ),
        output_details: make_output_details(
            0,
            reasoning_tokens,
            accepted_prediction_tokens,
            rejected_prediction_tokens,
            parse_modality_breakdown_from_detail_object(output_details_obj),
        ),
        extra_body,
    })
}

pub(crate) fn parse_usage_from_messages_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj
        .get("usage")
        .or_else(|| obj.get("message").and_then(|v| v.get("usage")))?
        .as_object()?;
    let input_tokens = usage.get("input_tokens")?.as_u64()?;
    let output_tokens = usage.get("output_tokens")?.as_u64()?;
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.get("cache_read_tokens"))
        .or_else(|| usage.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.get("cache_creation_tokens"))
        .or_else(|| usage.get("cache_write_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tool_prompt_tokens = usage
        .get("tool_prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("reasoning_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let accepted_prediction_tokens = usage
        .get("accepted_prediction_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let rejected_prediction_tokens = usage
        .get("rejected_prediction_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let extra_body = split_usage_extra(usage, &["input_tokens", "output_tokens"]);
    Some(urp::Usage {
        input_tokens,
        output_tokens,
        input_details: make_input_details(
            0,
            cache_read_tokens,
            cache_creation_tokens,
            tool_prompt_tokens,
            None,
        ),
        output_details: make_output_details(
            0,
            reasoning_tokens,
            accepted_prediction_tokens,
            rejected_prediction_tokens,
            None,
        ),
        extra_body,
    })
}

pub(crate) fn parse_usage_from_gemini_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj.get("usageMetadata")?.as_object()?;
    let input_tokens = usage
        .get("promptTokenCount")
        .or_else(|| usage.get("prompt_token_count"))
        .and_then(|v| v.as_u64())?;
    let output_tokens = usage
        .get("candidatesTokenCount")
        .or_else(|| usage.get("candidates_token_count"))
        .and_then(|v| v.as_u64())?;
    let cache_read_tokens = usage
        .get("cachedContentTokenCount")
        .or_else(|| usage.get("cached_content_token_count"))
        .or_else(|| usage.get("cached_tokens"))
        .or_else(|| usage.get("cache_read_tokens"))
        .or_else(|| usage.get("cache_read_input_tokens"))
        .or_else(|| usage.get("cacheReadInputTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cacheCreationInputTokens")
        .or_else(|| usage.get("cache_creation_input_tokens"))
        .or_else(|| usage.get("cache_creation_tokens"))
        .or_else(|| usage.get("cache_write_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tool_prompt_tokens = usage
        .get("toolPromptTokenCount")
        .or_else(|| usage.get("tool_prompt_token_count"))
        .or_else(|| usage.get("tool_prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("thoughtsTokenCount")
        .or_else(|| usage.get("thoughts_token_count"))
        .or_else(|| usage.get("reasoning_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let accepted_prediction_tokens = usage
        .get("acceptedPredictionTokenCount")
        .or_else(|| usage.get("accepted_prediction_token_count"))
        .or_else(|| usage.get("accepted_prediction_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let rejected_prediction_tokens = usage
        .get("rejectedPredictionTokenCount")
        .or_else(|| usage.get("rejected_prediction_token_count"))
        .or_else(|| usage.get("rejected_prediction_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let extra_body = split_usage_extra(
        usage,
        &[
            "promptTokenCount",
            "prompt_token_count",
            "candidatesTokenCount",
            "candidates_token_count",
        ],
    );
    Some(urp::Usage {
        input_tokens,
        output_tokens,
        input_details: make_input_details(
            0,
            cache_read_tokens,
            cache_creation_tokens,
            tool_prompt_tokens,
            None,
        ),
        output_details: make_output_details(
            0,
            reasoning_tokens,
            accepted_prediction_tokens,
            rejected_prediction_tokens,
            None,
        ),
        extra_body,
    })
}

pub(super) fn parse_usage_from_embeddings_object(obj: &Value) -> Option<urp::Usage> {
    let usage = obj.get("usage")?.as_object()?;
    let input_tokens = usage.get("prompt_tokens")?.as_u64()?;
    let total_tokens = usage.get("total_tokens")?.as_u64()?;
    let mut extra_body = HashMap::new();
    extra_body.insert("total_tokens".to_string(), Value::from(total_tokens));
    Some(urp::Usage {
        input_tokens,
        output_tokens: 0,
        input_details: None,
        output_details: None,
        extra_body,
    })
}

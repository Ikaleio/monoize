use crate::urp::{InputDetails, OutputDetails, Usage, UsageIteration, is_internal_key};
use serde_json::{Map, Value};
use std::collections::HashMap;

const KNOWN_USAGE_KEYS: &[&str] = &[
    "iterations",
    "input_tokens",
    "prompt_tokens",
    "output_tokens",
    "completion_tokens",
    "cache_read_input_tokens",
    "cache_read_tokens",
    "cached_tokens",
    "cache_creation_input_tokens",
    "cache_creation_tokens",
    "cache_write_tokens",
    "cache_creation",
    "tool_prompt_tokens",
    "tool_prompt_input_tokens",
    "reasoning_tokens",
    "reasoning_output_tokens",
    "output_tokens_details",
    "accepted_prediction_tokens",
    "accepted_prediction_output_tokens",
    "rejected_prediction_tokens",
    "rejected_prediction_output_tokens",
];

/// Messages streams report cumulative usage on `message_start` and `message_delta`. Each
/// reported counter replaces the previous value; unknown members merge recursively.
#[derive(Debug, Default)]
pub(super) struct AnthropicStreamUsageAccumulator {
    iterations: Option<Vec<UsageIteration>>,
    saw_usage: bool,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    cache_creation_5m_tokens: Option<u64>,
    cache_creation_1h_tokens: Option<u64>,
    tool_prompt_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    accepted_prediction_tokens: Option<u64>,
    rejected_prediction_tokens: Option<u64>,
    extra_body: HashMap<String, Value>,
}

impl AnthropicStreamUsageAccumulator {
    /// Merges the usage object of one Messages event and returns the cumulative snapshot,
    /// or `None` when the event carries no usage object.
    pub(super) fn merge_event(&mut self, event: &Value) -> Option<Usage> {
        let usage = event
            .get("usage")
            .or_else(|| {
                event
                    .get("message")
                    .and_then(|message| message.get("usage"))
            })?
            .as_object()?;
        self.saw_usage = true;
        if usage.contains_key("iterations") {
            self.iterations =
                crate::urp::usage::decode_messages_iterations(usage.get("iterations"));
        }

        replace_counter(
            usage,
            &["input_tokens", "prompt_tokens"],
            &mut self.input_tokens,
        );
        replace_counter(
            usage,
            &["output_tokens", "completion_tokens"],
            &mut self.output_tokens,
        );
        replace_counter(
            usage,
            &[
                "cache_read_input_tokens",
                "cache_read_tokens",
                "cached_tokens",
            ],
            &mut self.cache_read_tokens,
        );
        replace_counter(
            usage,
            &[
                "cache_creation_input_tokens",
                "cache_creation_tokens",
                "cache_write_tokens",
            ],
            &mut self.cache_creation_tokens,
        );
        replace_counter(
            usage,
            &["tool_prompt_tokens", "tool_prompt_input_tokens"],
            &mut self.tool_prompt_tokens,
        );
        replace_counter(
            usage,
            &["reasoning_tokens", "reasoning_output_tokens"],
            &mut self.reasoning_tokens,
        );
        replace_counter(
            usage,
            &[
                "accepted_prediction_tokens",
                "accepted_prediction_output_tokens",
            ],
            &mut self.accepted_prediction_tokens,
        );
        replace_counter(
            usage,
            &[
                "rejected_prediction_tokens",
                "rejected_prediction_output_tokens",
            ],
            &mut self.rejected_prediction_tokens,
        );

        if let Some(cache_creation) = usage.get("cache_creation").and_then(Value::as_object) {
            replace_counter(
                cache_creation,
                &["ephemeral_5m_input_tokens"],
                &mut self.cache_creation_5m_tokens,
            );
            replace_counter(
                cache_creation,
                &["ephemeral_1h_input_tokens"],
                &mut self.cache_creation_1h_tokens,
            );
        }
        if let Some(output_details) = usage
            .get("output_tokens_details")
            .and_then(Value::as_object)
        {
            replace_counter(
                output_details,
                &["thinking_tokens"],
                &mut self.reasoning_tokens,
            );
            let unknown_output_details: Map<String, Value> = output_details
                .iter()
                .filter(|(key, _)| key.as_str() != "thinking_tokens" && !is_internal_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !unknown_output_details.is_empty() {
                self.merge_extra(
                    "output_tokens_details",
                    &Value::Object(unknown_output_details),
                );
            }
        }

        for (key, value) in usage {
            if !KNOWN_USAGE_KEYS.contains(&key.as_str()) && !is_internal_key(key) {
                self.merge_extra(key, value);
            }
        }

        self.snapshot()
    }

    fn merge_extra(&mut self, key: &str, incoming: &Value) {
        match self.extra_body.get_mut(key) {
            Some(existing) => merge_cumulative_value(existing, incoming),
            None => {
                self.extra_body.insert(key.to_string(), incoming.clone());
            }
        }
    }

    pub(super) fn snapshot(&self) -> Option<Usage> {
        if !self.saw_usage {
            return None;
        }

        let cache_read_tokens = self.cache_read_tokens.unwrap_or(0);
        let cache_creation_tokens = self.cache_creation_tokens.unwrap_or(0);
        let cache_creation_5m_tokens = self.cache_creation_5m_tokens.unwrap_or(0);
        let cache_creation_1h_tokens = self.cache_creation_1h_tokens.unwrap_or(0);
        let tool_prompt_tokens = self.tool_prompt_tokens.unwrap_or(0);
        let reasoning_tokens = self.reasoning_tokens.unwrap_or(0);
        let accepted_prediction_tokens = self.accepted_prediction_tokens.unwrap_or(0);
        let rejected_prediction_tokens = self.rejected_prediction_tokens.unwrap_or(0);

        let input_details = (cache_read_tokens > 0
            || cache_creation_tokens > 0
            || cache_creation_5m_tokens > 0
            || cache_creation_1h_tokens > 0
            || tool_prompt_tokens > 0)
            .then_some(InputDetails {
                tool_prompt_modality_breakdown: None,
                standard_tokens: 0,
                cache_read_tokens,
                cache_read_modality_breakdown: None,
                cache_creation_tokens,
                cache_creation_5m_tokens,
                cache_creation_1h_tokens,
                tool_prompt_tokens,
                modality_breakdown: None,
            });
        let output_details = (reasoning_tokens > 0
            || accepted_prediction_tokens > 0
            || rejected_prediction_tokens > 0)
            .then_some(OutputDetails {
                standard_tokens: 0,
                reasoning_tokens,
                accepted_prediction_tokens,
                rejected_prediction_tokens,
                modality_breakdown: None,
            });

        Some(Usage {
            iterations: self.iterations.clone(),
            // Internal input tokens include the cache buckets that wire `input_tokens` excludes.
            input_tokens: self
                .input_tokens
                .unwrap_or(0)
                .saturating_add(cache_read_tokens)
                .saturating_add(cache_creation_tokens),
            output_tokens: self.output_tokens.unwrap_or(0),
            input_details,
            output_details,
            extra_body: self.extra_body.clone(),
        })
    }
}

fn replace_counter(object: &Map<String, Value>, keys: &[&str], destination: &mut Option<u64>) {
    if let Some(value) = keys
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_u64))
    {
        *destination = Some(value);
    }
}

fn merge_cumulative_value(existing: &mut Value, incoming: &Value) {
    if incoming.is_null() {
        return;
    }
    if let (Some(existing), Some(incoming)) = (existing.as_object_mut(), incoming.as_object()) {
        for (key, value) in incoming {
            match existing.get_mut(key) {
                Some(existing_value) => merge_cumulative_value(existing_value, value),
                None => {
                    existing.insert(key.clone(), value.clone());
                }
            }
        }
    } else {
        *existing = incoming.clone();
    }
}

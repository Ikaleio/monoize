//! Request `thinking` / `output_config.effort` generation and validation (RC4*).

use super::model::{
    generated_disabled_thinking_type, model_rejects_forced_tool_choice_with_adaptive,
    model_rejects_nondefault_sampling, model_supports_adaptive, model_supports_xhigh_effort,
};
use crate::urp::{
    MESSAGES_OUTPUT_CONFIG_EXTRA_KEY, MESSAGES_THINKING_CONFIG_EXTRA_KEY, ReasoningConfig,
};
use serde_json::{Map, Value, json};

/// Writes `thinking` and `output_config` for one request. Native Messages controls from the
/// client are the base; typed reasoning fields own `type`, `budget_tokens`, `display`, and
/// `effort`.
pub(super) fn encode_thinking_controls(
    obj: &mut Map<String, Value>,
    reasoning: &ReasoningConfig,
    upstream_model: &str,
) {
    let native_thinking = reasoning
        .extra_body
        .get(MESSAGES_THINKING_CONFIG_EXTRA_KEY)
        .and_then(Value::as_object);
    let native_output = reasoning
        .extra_body
        .get(MESSAGES_OUTPUT_CONFIG_EXTRA_KEY)
        .and_then(Value::as_object);
    let mut thinking = native_thinking.cloned().unwrap_or_default();
    let mut output = native_output.cloned().unwrap_or_default();
    thinking.retain(|key, _| !matches!(key.as_str(), "type" | "budget_tokens" | "display"));
    output.remove("effort");
    if output
        .get("format")
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        == Some("json_schema")
    {
        output.remove("format");
    }
    let explicit_thinking = native_thinking.is_some();
    let budget = reasoning.budget_tokens;
    let wants_summary = reasoning
        .summary
        .as_deref()
        .is_some_and(|summary| summary != "none");
    let mode: Option<&str> = if explicit_thinking {
        if reasoning.disabled() {
            Some("disabled")
        } else {
            reasoning.mode.as_deref()
        }
    } else if reasoning.disabled() {
        generated_disabled_thinking_type(upstream_model)
    } else if let Some(mode) = reasoning.mode.as_deref() {
        Some(mode)
    } else if budget.is_none()
        && reasoning.effort.is_none()
        && native_output.is_none()
        && wants_summary
        && model_supports_adaptive(upstream_model)
    {
        // RC4c.2: a summary request needs a thinking object to carry `display`.
        Some("adaptive")
    } else if (budget.is_none() && reasoning.effort.is_none())
        || (native_output.is_some() && budget.is_none())
    {
        None
    } else if model_supports_adaptive(upstream_model) {
        Some("adaptive")
    } else {
        Some("enabled")
    };
    if let Some(mode) = mode {
        thinking.insert("type".into(), json!(mode));
    }
    match budget {
        Some(budget) if explicit_thinking || mode == Some("enabled") => {
            thinking.insert("budget_tokens".into(), json!(budget));
        }
        None if !explicit_thinking && mode == Some("enabled") => {
            thinking.insert(
                "budget_tokens".into(),
                json!(effort_to_budget(
                    reasoning.effort.as_deref().unwrap_or("medium")
                )),
            );
        }
        _ => {}
    }
    if let Some(display) = &reasoning.display {
        thinking.insert("display".into(), json!(display));
    } else if !explicit_thinking
        && matches!(mode, Some("adaptive" | "enabled"))
        && model_supports_adaptive(upstream_model)
        && wants_summary
    {
        // Claude 5+ defaults display to "omitted"; a summary request needs it (RC4c.2).
        thinking.insert("display".into(), json!("summarized"));
    }
    let effort = reasoning
        .effort
        .as_deref()
        .filter(|v| !matches!(*v, "none" | "minimal" | "minimum"))
        .or_else(|| {
            budget
                .filter(|_| {
                    !explicit_thinking && native_output.is_none() && mode == Some("adaptive")
                })
                .map(|budget| effort_from_budget(budget, upstream_model))
        });
    if let Some(effort) = effort
        && (mode == Some("adaptive") || native_output.is_some())
    {
        output.insert("effort".into(), json!(effort));
    }
    if !thinking.is_empty() || native_thinking.is_some() {
        obj.insert("thinking".into(), Value::Object(thinking));
    }
    if !output.is_empty() || native_output.is_some() {
        obj.insert("output_config".into(), Value::Object(output));
    }
}

/// Rejects provider-invalid thinking controls that Monoize generated. Explicit client
/// Messages `thinking` is forwarded unvalidated (RC4d, RC4f).
pub(super) fn validate_generated_thinking_controls(
    obj: &Map<String, Value>,
    upstream_model: &str,
    explicit_thinking: bool,
    explicit_output_config: bool,
) -> Result<(), String> {
    if explicit_thinking {
        return Ok(());
    }
    let Some(thinking) = obj.get("thinking").and_then(Value::as_object) else {
        return Ok(());
    };
    match thinking.get("type").and_then(Value::as_str) {
        Some("enabled") => {
            let budget = thinking
                .get("budget_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    "Messages thinking.type=enabled requires integer budget_tokens".to_string()
                })?;
            if budget < 1024 {
                return Err("Messages thinking budget_tokens must be at least 1024".to_string());
            }
            let max_tokens = obj
                .get("max_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| "Messages max_tokens must be a non-negative integer".to_string())?;
            if budget >= max_tokens {
                return Err(format!(
                    "Messages thinking budget_tokens ({budget}) must be less than max_tokens ({max_tokens})"
                ));
            }
            if tool_choice_forces_use(obj.get("tool_choice")) {
                return Err(
                    "Messages manual thinking is incompatible with forced tool_choice type any or tool"
                        .to_string(),
                );
            }
        }
        Some("adaptive") => {
            if tool_choice_forces_use(obj.get("tool_choice"))
                && model_rejects_forced_tool_choice_with_adaptive(upstream_model)
            {
                return Err(format!(
                    "Messages model {upstream_model} does not support forced tool_choice with thinking"
                ));
            }
            if !explicit_output_config
                && obj
                    .get("output_config")
                    .and_then(|config| config.get("effort"))
                    .and_then(Value::as_str)
                    == Some("xhigh")
                && !model_supports_xhigh_effort(upstream_model)
            {
                return Err(format!(
                    "Messages model {upstream_model} does not support output_config.effort=xhigh"
                ));
            }
        }
        _ => return Ok(()),
    }

    if let Some(temperature) = obj.get("temperature")
        && temperature.as_f64() != Some(1.0)
    {
        return Err(
            "Messages temperature must be omitted or equal to 1 when thinking is active"
                .to_string(),
        );
    }
    if obj.contains_key("top_k") {
        return Err("Messages top_k must be omitted when thinking is active".to_string());
    }
    if let Some(top_p) = obj.get("top_p") {
        // Current models default top_p to 0.99 and answer every other value with HTTP 400.
        let allowed = if model_rejects_nondefault_sampling(upstream_model) {
            top_p.as_f64() == Some(0.99)
        } else {
            top_p
                .as_f64()
                .is_some_and(|top_p| (0.95..=1.0).contains(&top_p))
        };
        if !allowed {
            return Err(format!(
                "Messages top_p value {top_p} is not accepted with thinking on model {upstream_model}"
            ));
        }
    }
    if obj
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("assistant")
    {
        return Err("Messages thinking is incompatible with assistant prefill".to_string());
    }
    Ok(())
}

fn tool_choice_forces_use(tool_choice: Option<&Value>) -> bool {
    match tool_choice {
        Some(Value::Object(choice)) => matches!(
            choice.get("type").and_then(Value::as_str),
            Some("any" | "tool")
        ),
        Some(Value::String(choice)) => matches!(choice.as_str(), "any" | "required" | "tool"),
        _ => false,
    }
}

/// Fixed budget table of non-adaptive models. `xhigh` and `max` share a budget; their
/// distinction only surfaces as adaptive `output_config.effort`.
fn effort_to_budget(effort: &str) -> u32 {
    match effort {
        "minimum" | "minimal" | "low" => 1024,
        "medium" => 4096,
        "high" => 16384,
        "xhigh" | "max" => 32000,
        _ => 4096,
    }
}

/// RC4: generated adaptive effort for a bare thinking budget.
fn effort_from_budget(budget: u64, model: &str) -> &'static str {
    match budget {
        0..=1024 => "low",
        1025..=4096 => "medium",
        4097..=16384 => "high",
        _ if model_supports_xhigh_effort(model) => "xhigh",
        _ => "max",
    }
}

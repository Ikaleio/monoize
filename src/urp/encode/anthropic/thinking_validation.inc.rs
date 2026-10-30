/// Encodes one Messages request and rejects provider-invalid thinking controls that Monoize
/// generated before the caller can dispatch the body upstream. Explicit client Messages
/// controls are forwarded unvalidated (RC4d, RC4f).
pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    let mut prepared = crate::urp::media::prepare_request(req, ProviderProtocol::Messages)?;
    prepare_schema_custom_tools(&mut prepared)?;
    crate::urp::cache_breakpoint::materialize_wire_markers(
        &mut prepared,
        crate::urp::cache_breakpoint::CacheWireTarget::Messages,
        upstream_model,
    );
    validate_complete_tool_inputs(&prepared.input)?;
    let body = encode_prepared_request(&prepared, upstream_model);
    let explicit = |key: &str| {
        prepared
            .reasoning
            .as_ref()
            .is_some_and(|reasoning| reasoning.extra_body.contains_key(key))
    };
    validate_generated_thinking_controls(
        &body,
        upstream_model,
        explicit(MESSAGES_THINKING_CONFIG_EXTRA_KEY),
        explicit(MESSAGES_OUTPUT_CONFIG_EXTRA_KEY),
    )?;
    Ok(body)
}

fn validate_generated_thinking_controls(
    body: &Value,
    upstream_model: &str,
    explicit_thinking: bool,
    explicit_output_config: bool,
) -> Result<(), String> {
    if explicit_thinking {
        return Ok(());
    }
    let Some(obj) = body.as_object() else {
        return Ok(());
    };
    let Some(thinking) = obj.get("thinking").and_then(Value::as_object) else {
        return Ok(());
    };
    let thinking_type = thinking.get("type").and_then(Value::as_str);
    match thinking_type {
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
                && body.pointer("/output_config/effort").and_then(Value::as_str) == Some("xhigh")
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
        return Err("Messages temperature must be omitted or equal to 1 when thinking is active".to_string());
    }
    if obj.contains_key("top_k") {
        return Err("Messages top_k must be omitted when thinking is active".to_string());
    }
    if let Some(top_p) = obj.get("top_p") {
        // Current models default top_p to 0.99 and answer every other value with HTTP 400.
        let allowed = if model_rejects_nondefault_sampling(upstream_model) {
            top_p.as_f64() == Some(0.99)
        } else {
            top_p.as_f64().is_some_and(|top_p| (0.95..=1.0).contains(&top_p))
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

fn denotes_claude_model(model: &str) -> bool {
    model.contains("claude")
        || ["opus-", "sonnet-", "haiku-"]
            .iter()
            .any(|prefix| model.starts_with(prefix))
}

fn family_at_least(model: &str, family: &str, minimum: (u32, u32)) -> bool {
    claude_family_version(model, family).is_some_and(|version| version >= minimum)
}

/// RC4d.1: models that answer `thinking.type = "disabled"` with HTTP 400.
fn model_rejects_disabled_thinking(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (model.contains("fable")
            || model.contains("mythos")
            || family_at_least(&model, "opus", (5, 5))
            || family_at_least(&model, "sonnet", (5, 5)))
}

/// RC4: the thinking type generated for normalized effort `none`.
fn generated_disabled_thinking_type(model: &str) -> Option<&'static str> {
    if !model_rejects_disabled_thinking(model) {
        return Some("disabled");
    }
    // Sonnet 5.5 turns up-front thinking off with `between_tools`; other always-on
    // models think without configuration, so the field is omitted.
    (claude_family_version(&model.to_lowercase(), "sonnet") == Some((5, 5)))
        .then_some("between_tools")
}

/// RC4d.1: models that accept `output_config.effort = "xhigh"`.
fn model_supports_xhigh_effort(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (model.contains("fable")
            || (model.contains("mythos") && !model.contains("preview"))
            || family_at_least(&model, "opus", (4, 7))
            || family_at_least(&model, "sonnet", (5, 0))
            || family_at_least(&model, "haiku", (5, 5)))
}

/// RC4d.1: models that answer non-default sampling values with HTTP 400.
fn model_rejects_nondefault_sampling(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (model.contains("fable")
            || model.contains("mythos")
            || family_at_least(&model, "opus", (4, 7))
            || family_at_least(&model, "sonnet", (5, 0))
            || family_at_least(&model, "haiku", (5, 5)))
}

/// RC4d.1: adaptive-thinking models that reject forced tool use.
fn model_rejects_forced_tool_choice_with_adaptive(model: &str) -> bool {
    let model = model.to_lowercase();
    denotes_claude_model(&model)
        && (family_at_least(&model, "fable", (5, 1))
            || family_at_least(&model, "mythos", (5, 1))
            || family_at_least(&model, "opus", (5, 5))
            || family_at_least(&model, "sonnet", (5, 5)))
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

/// RC4g: default `max_tokens`, bounded by the model's documented synchronous output limit.
fn messages_default_max_tokens(model: &str) -> u64 {
    let model = model.to_lowercase();
    if !denotes_claude_model(&model) || model.contains("fable") || model.contains("mythos") {
        return 128_000;
    }
    if let Some(version) = claude_family_version(&model, "opus") {
        return match version {
            v if v >= (4, 6) => 128_000,
            v if v >= (4, 5) => 64_000,
            v if v >= (4, 0) => 32_000,
            _ => 4_096,
        };
    }
    if let Some(version) = claude_family_version(&model, "sonnet") {
        return match version {
            v if v >= (4, 6) => 128_000,
            v if v >= (3, 7) => 64_000,
            v if v >= (3, 5) => 8_192,
            _ => 4_096,
        };
    }
    if let Some(version) = claude_family_version(&model, "haiku") {
        return match version {
            v if v >= (5, 5) => 128_000,
            v if v >= (4, 0) => 64_000,
            v if v >= (3, 5) => 8_192,
            _ => 4_096,
        };
    }
    128_000
}

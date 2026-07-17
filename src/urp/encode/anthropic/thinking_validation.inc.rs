/// Encodes one Messages request and rejects provider-invalid thinking combinations before the
/// caller can dispatch the body upstream.
pub fn encode_request_checked(req: &UrpRequest, upstream_model: &str) -> Result<Value, String> {
    let body = encode_request(req, upstream_model);
    validate_outbound_thinking_request(&body, upstream_model)?;
    Ok(body)
}

fn validate_outbound_thinking_request(body: &Value, upstream_model: &str) -> Result<(), String> {
    let obj = body
        .as_object()
        .ok_or_else(|| "Messages request body must be an object".to_string())?;

    if model_rejects_nondefault_sampling(upstream_model) {
        validate_default_temperature(obj)?;
        validate_absent_top_k(obj)?;
        if let Some(top_p) = obj.get("top_p") {
            let top_p = top_p.as_f64().ok_or_else(|| {
                "Messages top_p must be numeric for this adaptive-thinking model".to_string()
            })?;
            if top_p != 1.0 {
                return Err(
                    "Messages top_p must be omitted or equal to 1 for this adaptive-thinking model"
                        .to_string(),
                );
            }
        }
    }

    let Some(thinking_value) = obj.get("thinking") else {
        return Ok(());
    };
    let thinking = thinking_value
        .as_object()
        .ok_or_else(|| "Messages thinking must be an object".to_string())?;
    let thinking_type = thinking
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "Messages thinking.type must be a string".to_string())?;

    match thinking_type {
        "disabled" => {
            if model_has_always_on_adaptive_thinking(upstream_model) {
                return Err(format!(
                    "Messages model {upstream_model} does not support thinking.type=disabled"
                ));
            }
            if thinking.contains_key("budget_tokens") {
                return Err(
                    "Messages thinking.type=disabled must not include budget_tokens".to_string(),
                );
            }
            if thinking.contains_key("display") {
                return Err("Messages thinking.type=disabled must not include display".to_string());
            }
            return Ok(());
        }
        "enabled" => {
            validate_thinking_display(thinking)?;
            validate_manual_thinking(thinking, obj, upstream_model)?;
        }
        "adaptive" => {
            validate_thinking_display(thinking)?;
            validate_adaptive_thinking(thinking, obj, upstream_model)?;
        }
        other => {
            return Err(format!(
                "Messages thinking.type must be enabled, adaptive, or disabled; got {other}"
            ));
        }
    }

    validate_default_temperature(obj)?;
    validate_absent_top_k(obj)?;
    if let Some(top_p) = obj.get("top_p") {
        let top_p = top_p
            .as_f64()
            .ok_or_else(|| "Messages top_p must be numeric when thinking is active".to_string())?;
        if !(0.95..=1.0).contains(&top_p) {
            return Err(
                "Messages top_p must be between 0.95 and 1 when thinking is active".to_string(),
            );
        }
    }
    if tool_choice_forces_use(obj.get("tool_choice")) {
        return Err(
            "Messages thinking is incompatible with forced tool_choice type any or tool"
                .to_string(),
        );
    }
    if obj
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .and_then(Value::as_object)
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("assistant")
    {
        return Err("Messages thinking is incompatible with assistant prefill".to_string());
    }

    Ok(())
}

fn validate_manual_thinking(
    thinking: &Map<String, Value>,
    request: &Map<String, Value>,
    upstream_model: &str,
) -> Result<(), String> {
    if model_requires_adaptive_thinking(upstream_model) {
        return Err(format!(
            "Messages model {upstream_model} does not support thinking.type=enabled; use adaptive"
        ));
    }
    let budget = thinking
        .get("budget_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            "Messages thinking.type=enabled requires integer budget_tokens".to_string()
        })?;
    if budget < 1024 {
        return Err("Messages thinking budget_tokens must be at least 1024".to_string());
    }
    let max_tokens = request
        .get("max_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Messages max_tokens must be a non-negative integer".to_string())?;
    if budget >= max_tokens {
        return Err(format!(
            "Messages thinking budget_tokens ({budget}) must be less than max_tokens ({max_tokens})"
        ));
    }
    Ok(())
}

fn validate_adaptive_thinking(
    thinking: &Map<String, Value>,
    request: &Map<String, Value>,
    upstream_model: &str,
) -> Result<(), String> {
    if !model_supports_adaptive(upstream_model) {
        return Err(format!(
            "Messages model {upstream_model} does not support thinking.type=adaptive"
        ));
    }
    if thinking.contains_key("budget_tokens") {
        return Err("Messages thinking.type=adaptive must not include budget_tokens".to_string());
    }
    if let Some(effort) = request
        .get("output_config")
        .and_then(Value::as_object)
        .and_then(|config| config.get("effort"))
    {
        let effort = effort.as_str().ok_or_else(|| {
            "Messages output_config.effort must be a string with adaptive thinking".to_string()
        })?;
        if !matches!(effort, "low" | "medium" | "high" | "xhigh" | "max") {
            return Err(format!(
                "Messages output_config.effort is invalid for adaptive thinking: {effort}"
            ));
        }
        if effort == "xhigh" && !model_supports_xhigh_effort(upstream_model) {
            return Err(format!(
                "Messages model {upstream_model} does not support output_config.effort=xhigh"
            ));
        }
    }
    Ok(())
}

fn validate_thinking_display(thinking: &Map<String, Value>) -> Result<(), String> {
    let Some(display) = thinking.get("display") else {
        return Ok(());
    };
    if matches!(display.as_str(), Some("summarized" | "omitted")) {
        return Ok(());
    }
    Err("Messages thinking.display must be summarized or omitted".to_string())
}

fn validate_default_temperature(request: &Map<String, Value>) -> Result<(), String> {
    let Some(temperature) = request.get("temperature") else {
        return Ok(());
    };
    let temperature = temperature
        .as_f64()
        .ok_or_else(|| "Messages temperature must be numeric".to_string())?;
    if temperature != 1.0 {
        return Err("Messages temperature must be omitted or equal to 1".to_string());
    }
    Ok(())
}

fn validate_absent_top_k(request: &Map<String, Value>) -> Result<(), String> {
    if request.contains_key("top_k") {
        return Err("Messages top_k must be omitted for this request".to_string());
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

fn model_requires_adaptive_thinking(model: &str) -> bool {
    let model = model.to_lowercase();
    if !model_supports_adaptive(&model) {
        return false;
    }
    if model.contains("fable") || (model.contains("mythos") && !model.contains("preview")) {
        return true;
    }
    claude_family_version(&model, "opus").is_some_and(|version| version >= (4, 7))
        || claude_family_version(&model, "sonnet").is_some_and(|version| version >= (5, 0))
}

fn model_has_always_on_adaptive_thinking(model: &str) -> bool {
    let model = model.to_lowercase();
    model_supports_adaptive(&model) && (model.contains("fable") || model.contains("mythos"))
}

fn model_rejects_nondefault_sampling(model: &str) -> bool {
    let model = model.to_lowercase();
    if !model_supports_adaptive(&model) {
        return false;
    }
    if model.contains("fable") || model.contains("mythos") {
        return true;
    }
    claude_family_version(&model, "opus").is_some_and(|version| version >= (4, 7))
        || claude_family_version(&model, "sonnet").is_some_and(|version| version >= (5, 0))
}

fn model_supports_xhigh_effort(model: &str) -> bool {
    let model = model.to_lowercase();
    if !model_supports_adaptive(&model) {
        return false;
    }
    if model.contains("fable") || (model.contains("mythos") && !model.contains("preview")) {
        return true;
    }
    claude_family_version(&model, "opus").is_some_and(|version| version >= (4, 7))
        || claude_family_version(&model, "sonnet").is_some_and(|version| version >= (5, 0))
}

#[cfg(test)]
mod thinking_validation_tests {
    use super::*;
    use crate::urp::{Node, OrdinaryRole, ReasoningConfig, ToolChoice};

    fn request(model: &str, max_tokens: u64, effort: &str) -> UrpRequest {
        UrpRequest {
            model: model.to_string(),
            input: vec![Node::Text {
                id: None,
                role: OrdinaryRole::User,
                content: "reason".to_string(),
                phase: None,
                extra_body: HashMap::new(),
            }],
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: Some(max_tokens),
            reasoning: Some(ReasoningConfig {
                effort: Some(effort.to_string()),
                extra_body: HashMap::new(),
            }),
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        }
    }

    fn assert_invalid(mut req: UrpRequest, model: &str, expected: &str) {
        req.model = model.to_string();
        let error = encode_request_checked(&req, model).expect_err("request must be rejected");
        assert!(
            error.contains(expected),
            "unexpected validation error: {error}"
        );
    }

    #[test]
    fn checked_encoder_accepts_valid_manual_and_adaptive_thinking() {
        let manual = encode_request_checked(
            &request("claude-sonnet-4-5", 4096, "low"),
            "claude-sonnet-4-5",
        )
        .expect("valid manual thinking");
        assert_eq!(
            manual["thinking"],
            json!({ "type": "enabled", "budget_tokens": 1024 })
        );

        let adaptive =
            encode_request_checked(&request("claude-sonnet-5", 4096, "high"), "claude-sonnet-5")
                .expect("valid adaptive thinking");
        assert_eq!(adaptive["thinking"], json!({ "type": "adaptive" }));
        assert_eq!(adaptive["output_config"]["effort"], json!("high"));
    }

    #[test]
    fn checked_encoder_rejects_invalid_manual_budget_relationship() {
        assert_invalid(
            request("claude-sonnet-4-5", 1024, "low"),
            "claude-sonnet-4-5",
            "must be less than max_tokens",
        );
        let mut below_minimum = request("claude-sonnet-4-5", 4096, "low");
        below_minimum
            .reasoning
            .as_mut()
            .expect("reasoning")
            .extra_body
            .insert(
                MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
                json!({ "type": "enabled", "budget_tokens": 1023 }),
            );
        assert_invalid(below_minimum, "claude-sonnet-4-5", "must be at least 1024");
    }

    #[test]
    fn checked_encoder_rejects_active_thinking_incompatible_controls() {
        let base = request("claude-sonnet-4-5", 4096, "low");

        let mut temperature = base.clone();
        temperature.temperature = Some(0.5);
        assert_invalid(temperature, "claude-sonnet-4-5", "temperature");

        let mut top_k = base.clone();
        top_k.extra_body.insert("top_k".to_string(), json!(10));
        assert_invalid(top_k, "claude-sonnet-4-5", "top_k");

        let mut top_p = base.clone();
        top_p.top_p = Some(0.9);
        assert_invalid(top_p, "claude-sonnet-4-5", "top_p");

        let mut forced_tool = base.clone();
        forced_tool.tool_choice = Some(ToolChoice::Mode("required".to_string()));
        assert_invalid(forced_tool, "claude-sonnet-4-5", "forced tool_choice");

        let mut prefill = base;
        prefill.input.push(Node::Text {
            id: None,
            role: OrdinaryRole::Assistant,
            content: "prefix".to_string(),
            phase: None,
            extra_body: HashMap::new(),
        });
        assert_invalid(prefill, "claude-sonnet-4-5", "assistant prefill");
    }

    #[test]
    fn checked_encoder_enforces_adaptive_model_constraints() {
        let mut explicit_manual = request("claude-sonnet-5", 4096, "high");
        explicit_manual
            .reasoning
            .as_mut()
            .expect("reasoning")
            .extra_body
            .insert(
                MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
                json!({ "type": "enabled", "budget_tokens": 2048 }),
            );
        assert_invalid(
            explicit_manual,
            "claude-sonnet-5",
            "does not support thinking.type=enabled",
        );

        let mut strict_top_p = request("claude-sonnet-5", 4096, "high");
        strict_top_p.top_p = Some(0.95);
        assert_invalid(strict_top_p, "claude-sonnet-5", "top_p");

        let mut disabled_fable = request("claude-fable-5", 4096, "high");
        disabled_fable
            .reasoning
            .as_mut()
            .expect("reasoning")
            .extra_body
            .insert(
                MESSAGES_THINKING_CONFIG_EXTRA_KEY.to_string(),
                json!({ "type": "disabled" }),
            );
        assert_invalid(
            disabled_fable,
            "claude-fable-5",
            "does not support thinking.type=disabled",
        );

        let mut unsupported_xhigh = request("claude-sonnet-4-6", 4096, "xhigh");
        assert_invalid(
            unsupported_xhigh.clone(),
            "claude-sonnet-4-6",
            "does not support output_config.effort=xhigh",
        );
        unsupported_xhigh
            .reasoning
            .as_mut()
            .expect("reasoning")
            .effort = Some("max".to_string());
        encode_request_checked(&unsupported_xhigh, "claude-sonnet-4-6")
            .expect("max effort is valid for Sonnet 4.6 adaptive thinking");
    }
}

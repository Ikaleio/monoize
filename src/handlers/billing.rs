use super::*;

#[derive(Debug, Clone)]
pub(super) struct ChargeComponents {
    prompt_tokens: i128,
    completion_tokens: i128,
    cached_tokens: i128,
    cache_creation_tokens: i128,
    billed_cache_creation_tokens: i128,
    cache_creation_charge: i128,
    reasoning_tokens: i128,
    billed_uncached_prompt_tokens: i128,
    billed_cached_prompt_tokens: i128,
    billed_non_reasoning_completion_tokens: i128,
    billed_reasoning_completion_tokens: i128,
    uncached_prompt_charge: i128,
    cached_prompt_charge: i128,
    non_reasoning_completion_charge: i128,
    reasoning_completion_charge: i128,
    prompt_charge: i128,
    completion_charge: i128,
    base_charge: i128,
    final_charge: i128,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ChargeComputation {
    pub(super) charge_nano_usd: Option<i128>,
    pub(super) billing_breakdown: Option<Value>,
}

pub(super) fn non_negative_i128_to_u64(value: i128) -> u64 {
    if value <= 0 {
        0
    } else {
        u64::try_from(value).unwrap_or(u64::MAX)
    }
}

pub(super) fn parse_u64_value(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

pub(super) fn map_get_u64(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(parse_u64_value)
}

pub(super) fn calculate_charge_components(
    usage: &urp::Usage,
    pricing: &ModelPricing,
    provider_multiplier: f64,
    provider_type: crate::config::ProviderType,
) -> Option<ChargeComponents> {
    let prompt_tokens = i128::from(usage.input_tokens);
    let completion_tokens = i128::from(usage.output_tokens);
    let cached_tokens = i128::from(usage.cached_tokens().unwrap_or(0));
    let cache_creation_tokens =
        i128::from(usage.input_details.as_ref().map(|d| d.cache_creation_tokens).unwrap_or(0));
    let reasoning_tokens = i128::from(usage.reasoning_tokens().unwrap_or(0));

    let uncached_prompt_tokens = match provider_type {
        ProviderType::Messages => prompt_tokens.max(0),
        _ => (prompt_tokens - cached_tokens - cache_creation_tokens).max(0),
    };
    let non_reasoning_completion_tokens = (completion_tokens - reasoning_tokens).max(0);

    let (
        billed_uncached_prompt_tokens,
        billed_cached_prompt_tokens,
        uncached_prompt_charge,
        cached_prompt_charge,
    ) = if let Some(cached_rate) = pricing.cache_read_input_cost_per_token_nano {
        let uncached_charge =
            uncached_prompt_tokens.checked_mul(pricing.input_cost_per_token_nano)?;
        let cached_charge = cached_tokens.max(0).checked_mul(cached_rate)?;
        (
            uncached_prompt_tokens,
            cached_tokens.max(0),
            uncached_charge,
            cached_charge,
        )
    } else {
        (
            prompt_tokens.max(0),
            0,
            prompt_tokens.checked_mul(pricing.input_cost_per_token_nano)?,
            0,
        )
    };
    let prompt_charge = uncached_prompt_charge.checked_add(cached_prompt_charge)?;

    let (billed_cache_creation_tokens, cache_creation_charge) =
        if let Some(cache_creation_rate) = pricing.cache_creation_input_cost_per_token_nano {
            let tokens = cache_creation_tokens.max(0);
            let charge = tokens.checked_mul(cache_creation_rate)?;
            (tokens, charge)
        } else {
            (0, 0)
        };

    let (
        billed_non_reasoning_completion_tokens,
        billed_reasoning_completion_tokens,
        non_reasoning_completion_charge,
        reasoning_completion_charge,
    ) = if let Some(reasoning_rate) = pricing.output_cost_per_reasoning_token_nano {
        let non_reasoning_charge =
            non_reasoning_completion_tokens.checked_mul(pricing.output_cost_per_token_nano)?;
        let reasoning_charge = reasoning_tokens.max(0).checked_mul(reasoning_rate)?;
        (
            non_reasoning_completion_tokens,
            reasoning_tokens.max(0),
            non_reasoning_charge,
            reasoning_charge,
        )
    } else {
        (
            completion_tokens.max(0),
            0,
            completion_tokens.checked_mul(pricing.output_cost_per_token_nano)?,
            0,
        )
    };
    let completion_charge =
        non_reasoning_completion_charge.checked_add(reasoning_completion_charge)?;

    let base_charge = prompt_charge
        .checked_add(completion_charge)?
        .checked_add(cache_creation_charge)?;
    let final_charge = scale_charge_with_multiplier(base_charge, provider_multiplier)?;

    Some(ChargeComponents {
        prompt_tokens,
        completion_tokens,
        cached_tokens,
        cache_creation_tokens,
        billed_cache_creation_tokens,
        cache_creation_charge,
        reasoning_tokens,
        billed_uncached_prompt_tokens,
        billed_cached_prompt_tokens,
        billed_non_reasoning_completion_tokens,
        billed_reasoning_completion_tokens,
        uncached_prompt_charge,
        cached_prompt_charge,
        non_reasoning_completion_charge,
        reasoning_completion_charge,
        prompt_charge,
        completion_charge,
        base_charge,
        final_charge,
    })
}

#[cfg(test)]
pub(super) fn calculate_charge_nano(
    usage: &urp::Usage,
    pricing: &ModelPricing,
    provider_multiplier: f64,
    provider_type: crate::config::ProviderType,
) -> Option<i128> {
    calculate_charge_components(usage, pricing, provider_multiplier, provider_type)
        .map(|parts| parts.final_charge)
}

pub(super) fn build_usage_breakdown(usage: &urp::Usage) -> Value {
    let input_details = usage.input_details.as_ref();
    let output_details = usage.output_details.as_ref();

    let input_cached = input_details
        .map(|d| d.cache_read_tokens)
        .filter(|&v| v > 0)
        .or_else(|| {
            usage
                .extra_body
                .get("cache_read_input_tokens")
                .and_then(parse_u64_value)
        })
        .or_else(|| {
            usage
                .extra_body
                .get("input_tokens_details")
                .and_then(|v| v.as_object())
                .and_then(|d| map_get_u64(d, "cached_tokens"))
        })
        .or_else(|| {
            usage
                .extra_body
                .get("prompt_tokens_details")
                .and_then(|v| v.as_object())
                .and_then(|d| map_get_u64(d, "cached_tokens"))
        });
    let input_cache_creation = input_details
        .map(|d| d.cache_creation_tokens)
        .filter(|&v| v > 0)
        .or_else(|| {
            usage
                .extra_body
                .get("cache_creation_input_tokens")
                .and_then(parse_u64_value)
        });
    let input_text = input_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.text_tokens);
    let input_audio = input_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.audio_tokens);
    let input_image = input_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.image_tokens);
    let output_reasoning = output_details
        .map(|d| d.reasoning_tokens)
        .filter(|&v| v > 0)
        .or_else(|| {
            usage
                .extra_body
                .get("output_tokens_details")
                .and_then(|v| v.as_object())
                .and_then(|d| map_get_u64(d, "reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .extra_body
                .get("completion_tokens_details")
                .and_then(|v| v.as_object())
                .and_then(|d| map_get_u64(d, "reasoning_tokens"))
        });
    let output_text = output_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.text_tokens);
    let output_audio = output_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.audio_tokens);
    let output_image = output_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.image_tokens);

    json!({
        "version": 1,
        "input": {
            "total_tokens": usage.input_tokens,
            "uncached_tokens": usage.input_tokens.saturating_sub(input_cached.unwrap_or(0)),
            "text_tokens": input_text,
            "cached_tokens": input_cached,
            "cache_creation_tokens": input_cache_creation,
            "audio_tokens": input_audio,
            "image_tokens": input_image
        },
        "output": {
            "total_tokens": usage.output_tokens,
            "non_reasoning_tokens": usage.output_tokens.saturating_sub(output_reasoning.unwrap_or(0)),
            "text_tokens": output_text,
            "reasoning_tokens": output_reasoning,
            "audio_tokens": output_audio,
            "image_tokens": output_image
        },
        "raw_usage_extra": usage.extra_body
    })
}

pub(super) fn build_billing_breakdown(
    logical_model: &str,
    attempt: &MonoizeAttempt,
    pricing: &ModelPricing,
    components: &ChargeComponents,
) -> Value {
    json!({
        "version": 1,
        "currency": "nano_usd",
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "provider_id": attempt.provider_id,
        "provider_multiplier": attempt.model_multiplier,
        "input": {
            "total_tokens": non_negative_i128_to_u64(components.prompt_tokens),
            "cached_tokens": non_negative_i128_to_u64(components.cached_tokens),
            "billed_uncached_tokens": non_negative_i128_to_u64(components.billed_uncached_prompt_tokens),
            "billed_cached_tokens": non_negative_i128_to_u64(components.billed_cached_prompt_tokens),
            "unit_price_nano": pricing.input_cost_per_token_nano.to_string(),
            "cached_unit_price_nano": pricing.cache_read_input_cost_per_token_nano.map(|v| v.to_string()),
            "uncached_charge_nano": components.uncached_prompt_charge.to_string(),
            "cached_charge_nano": components.cached_prompt_charge.to_string(),
            "cache_creation_tokens": non_negative_i128_to_u64(components.cache_creation_tokens),
            "billed_cache_creation_tokens": non_negative_i128_to_u64(components.billed_cache_creation_tokens),
            "cache_creation_unit_price_nano": pricing.cache_creation_input_cost_per_token_nano.map(|v| v.to_string()),
            "cache_creation_charge_nano": components.cache_creation_charge.to_string(),
            "total_charge_nano": components.prompt_charge.to_string(),
        },
        "output": {
            "total_tokens": non_negative_i128_to_u64(components.completion_tokens),
            "reasoning_tokens": non_negative_i128_to_u64(components.reasoning_tokens),
            "billed_non_reasoning_tokens": non_negative_i128_to_u64(components.billed_non_reasoning_completion_tokens),
            "billed_reasoning_tokens": non_negative_i128_to_u64(components.billed_reasoning_completion_tokens),
            "unit_price_nano": pricing.output_cost_per_token_nano.to_string(),
            "reasoning_unit_price_nano": pricing.output_cost_per_reasoning_token_nano.map(|v| v.to_string()),
            "non_reasoning_charge_nano": components.non_reasoning_completion_charge.to_string(),
            "reasoning_charge_nano": components.reasoning_completion_charge.to_string(),
            "total_charge_nano": components.completion_charge.to_string(),
        },
        "base_charge_nano": components.base_charge.to_string(),
        "final_charge_nano": components.final_charge.to_string(),
    })
}

pub(super) fn scale_charge_with_multiplier(base_nano: i128, provider_multiplier: f64) -> Option<i128> {
    if !provider_multiplier.is_finite() || provider_multiplier < 0.0 {
        return None;
    }

    pub(super) const SCALE: i128 = 1_000_000_000;
    let multiplier_repr = format!("{provider_multiplier:.18}");
    let mut parts = multiplier_repr.split('.');
    let whole = parts.next().unwrap_or("0").parse::<i128>().ok()?;
    let frac_raw = parts.next().unwrap_or("0");
    let mut frac_nano = String::with_capacity(9);
    for ch in frac_raw.chars().take(9) {
        frac_nano.push(ch);
    }
    while frac_nano.len() < 9 {
        frac_nano.push('0');
    }
    let frac = frac_nano.parse::<i128>().ok()?;

    let multiplier_nano = whole.checked_mul(SCALE)?.checked_add(frac)?;
    base_nano.checked_mul(multiplier_nano)?.checked_div(SCALE)
}

pub(super) async fn maybe_charge_usage(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: &urp::Usage,
) -> AppResult<ChargeComputation> {
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(ChargeComputation::default());
    };
    let pricing = match state
        .model_registry_store
        .get_model_pricing(&attempt.upstream_model)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?
    {
        Some(v) => v,
        None => {
            return Err(AppError::new(
                StatusCode::FORBIDDEN,
                "model_pricing_required",
                format!("pricing metadata required for model: {}", attempt.upstream_model),
            ));
        }
    };

    let Some(components) = calculate_charge_components(
        usage,
        &pricing,
        attempt.model_multiplier,
        attempt.provider_type,
    )
    else {
        tracing::error!(
            "billing error: charge overflow for model={}",
            attempt.upstream_model
        );
        return Err(AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "billing_overflow",
            format!("charge calculation overflow for model={}", attempt.upstream_model),
        ));
    };
    let billing_breakdown = build_billing_breakdown(logical_model, attempt, &pricing, &components);
    let charge_nano = components.final_charge;
    if charge_nano <= 0 {
        return Ok(ChargeComputation {
            charge_nano_usd: None,
            billing_breakdown: Some(billing_breakdown),
        });
    }

    let meta = json!({
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "provider_id": attempt.provider_id,
        "provider_multiplier": attempt.model_multiplier,
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
        "cached_tokens": usage.cached_tokens(),
        "cache_creation_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_tokens),
        "reasoning_tokens": usage.reasoning_tokens(),
        "charge_nano_usd": charge_nano.to_string(),
    });

    match state
        .user_store
        .charge_user_balance_nano(user_id, charge_nano, &meta)
        .await
    {
        Ok(()) => Ok(ChargeComputation {
            charge_nano_usd: Some(charge_nano),
            billing_breakdown: Some(billing_breakdown),
        }),
        Err(err) => match err.kind {
            BillingErrorKind::InsufficientBalance => Err(AppError::new(
                StatusCode::PAYMENT_REQUIRED,
                "insufficient_balance",
                "insufficient balance",
            )),
            BillingErrorKind::NotFound => Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "user not found",
            )),
            _ => Err(AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                err.message,
            )),
        },
    }
}

pub(super) async fn maybe_charge_response(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    response: &urp::UrpResponse,
) -> AppResult<ChargeComputation> {
    let Some(usage) = response.usage.as_ref() else {
        return Ok(ChargeComputation::default());
    };
    maybe_charge_usage(state, auth, attempt, logical_model, usage).await
}

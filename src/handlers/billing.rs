use super::*;
use crate::billing_rate_store::DbBillingRateRecord;
#[cfg(test)]
use crate::model_registry_store::ModelPricing;

#[derive(Debug, Clone)]
pub(super) struct BillingRateResolution {
    pub(super) pricing_profile: String,
    pub(super) pricing_model: String,
    pub(super) rates: Vec<DbBillingRateRecord>,
}

#[derive(Debug, Clone)]
pub(super) struct MatrixChargeComponents {
    pub(super) token_line_items: Vec<Value>,
    pub(super) meter_line_items: Vec<Value>,
    pub(super) context_tier: Option<String>,
    pub(super) service_tier: Option<String>,
    pub(super) base_charge: i128,
    pub(super) final_charge: i128,
}

#[derive(Debug, Clone)]
#[cfg(test)]
#[allow(dead_code)]
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

#[cfg(test)]
#[allow(dead_code)]
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

#[cfg(test)]
pub(super) fn calculate_charge_components(
    usage: &urp::Usage,
    pricing: &ModelPricing,
    provider_multiplier: f64,
) -> Option<ChargeComponents> {
    let prompt_tokens = i128::from(usage.input_tokens);
    let completion_tokens = i128::from(usage.output_tokens);
    let cached_tokens = i128::from(usage.cached_tokens().unwrap_or(0));
    let cache_creation_tokens = i128::from(
        usage
            .input_details
            .as_ref()
            .map(|d| d.cache_creation_tokens)
            .unwrap_or(0),
    );
    let reasoning_tokens = i128::from(usage.reasoning_tokens().unwrap_or(0));

    let uncached_prompt_tokens = (prompt_tokens - cached_tokens - cache_creation_tokens).max(0);
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
        // No cache-read pricing, but cache_creation tokens still MUST be excluded from
        // the base input bucket to avoid double-billing (spec § 5 C3a).
        (
            uncached_prompt_tokens,
            0,
            uncached_prompt_tokens.checked_mul(pricing.input_cost_per_token_nano)?,
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
#[cfg(test)]
pub(super) fn calculate_charge_nano(
    usage: &urp::Usage,
    pricing: &ModelPricing,
    provider_multiplier: f64,
) -> Option<i128> {
    calculate_charge_components(usage, pricing, provider_multiplier).map(|parts| parts.final_charge)
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
    let input_cache_creation_5m = input_details
        .map(|d| d.cache_creation_5m_tokens)
        .filter(|&v| v > 0);
    let input_cache_creation_1h = input_details
        .map(|d| d.cache_creation_1h_tokens)
        .filter(|&v| v > 0);
    let input_text = input_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.text_tokens);
    let input_cached_text = input_details
        .and_then(|d| d.cache_read_modality_breakdown.as_ref())
        .and_then(|m| m.text_tokens);
    let input_audio = input_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.audio_tokens);
    let input_cached_audio = input_details
        .and_then(|d| d.cache_read_modality_breakdown.as_ref())
        .and_then(|m| m.audio_tokens);
    let input_image = input_details
        .and_then(|d| d.modality_breakdown.as_ref())
        .and_then(|m| m.image_tokens);
    let input_cached_image = input_details
        .and_then(|d| d.cache_read_modality_breakdown.as_ref())
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
            "uncached_tokens": usage.input_tokens
                .saturating_sub(input_cached.unwrap_or(0))
                .saturating_sub(input_cache_creation.unwrap_or(0)),
            "text_tokens": input_text,
            "cached_text_tokens": input_cached_text,
            "cached_tokens": input_cached,
            "cache_creation_tokens": input_cache_creation,
            "cache_creation_5m_tokens": input_cache_creation_5m,
            "cache_creation_1h_tokens": input_cache_creation_1h,
            "audio_tokens": input_audio,
            "cached_audio_tokens": input_cached_audio,
            "image_tokens": input_image,
            "cached_image_tokens": input_cached_image
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

#[cfg(test)]
#[allow(dead_code)]
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

pub(super) async fn resolve_billing_rate_matrix(
    state: &AppState,
    upstream_model: &str,
    logical_model: &str,
    provider_type: ProviderType,
) -> AppResult<Option<BillingRateResolution>> {
    let normalized_upstream_model = normalized_pricing_model_key(state, upstream_model).await;
    if let Some(resolution) =
        resolve_billing_rate_matrix_for_model(state, &normalized_upstream_model, provider_type)
            .await?
    {
        return Ok(Some(resolution));
    }

    let normalized_logical_model = normalized_pricing_model_key(state, logical_model).await;
    if normalized_logical_model == normalized_upstream_model {
        return Ok(None);
    }
    resolve_billing_rate_matrix_for_model(state, &normalized_logical_model, provider_type).await
}

async fn resolve_billing_rate_matrix_for_model(
    state: &AppState,
    model: &str,
    provider_type: ProviderType,
) -> AppResult<Option<BillingRateResolution>> {
    let patterns = state
        .settings_store
        .get_pricing_profile_model_patterns()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let selected_profile = crate::billing_rate_store::select_pricing_profile(&patterns, model);
    let mut candidate_profiles = Vec::new();
    if let Some(pricing_profile) = selected_profile {
        candidate_profiles.push(pricing_profile.to_string());
    }
    if let Some(metadata_profile) = metadata_pricing_profile_for_model(state, model).await? {
        if !candidate_profiles
            .iter()
            .any(|candidate| candidate == &metadata_profile)
        {
            candidate_profiles.push(metadata_profile);
        }
    }
    if candidate_profiles.is_empty() {
        return Ok(None);
    }

    let provider_type_str = reasoning_envelope_provider_type(provider_type);
    let mut first_non_empty = None;
    for pricing_profile in candidate_profiles {
        let rates = state
            .billing_rate_store
            .list_matching_rates(&pricing_profile, Some(provider_type_str), model)
            .await
            .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
        if rates.is_empty() {
            continue;
        }
        let resolution = BillingRateResolution {
            pricing_profile,
            pricing_model: model.to_string(),
            rates,
        };
        if billing_rate_matrix_allows_request(&resolution, &[]).unwrap_or(false) {
            return Ok(Some(resolution));
        }
        if first_non_empty.is_none() {
            first_non_empty = Some(resolution);
        }
    }
    Ok(first_non_empty)
}

async fn metadata_pricing_profile_for_model(
    state: &AppState,
    model: &str,
) -> AppResult<Option<String>> {
    let record = state
        .model_registry_store
        .get_model_metadata(model)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    Ok(record
        .and_then(|record| record.models_dev_provider)
        .map(|profile| profile.trim().to_string())
        .filter(|profile| !profile.is_empty()))
}

pub(super) fn billing_rate_matrix_allows_request(
    resolution: &BillingRateResolution,
    server_tool_usage_classes: &[String],
) -> Result<bool, String> {
    let has_input = resolution
        .rates
        .iter()
        .any(|r| r.rate_kind == "token" && r.usage_class == "input_uncached");
    let has_output = resolution
        .rates
        .iter()
        .any(|r| r.rate_kind == "token" && r.usage_class == "output");
    if !has_input || !has_output {
        return Ok(false);
    }
    let context_tiers: std::collections::BTreeSet<String> = resolution
        .rates
        .iter()
        .filter_map(|r| r.context_tier.as_deref())
        .filter(|tier| *tier != "default")
        .map(str::to_string)
        .collect();
    if !context_tiers.is_empty() {
        let has_threshold = resolution
            .rates
            .iter()
            .filter_map(|r| r.match_json.get("context_threshold_tokens"))
            .any(|value| parse_u64_value(value).is_some());
        if !has_threshold {
            return Err("context-tier rate requires context_threshold_tokens".to_string());
        }
        for tier in &context_tiers {
            for usage_class in ["input_uncached", "output"] {
                let has_tier_rate = resolution.rates.iter().any(|r| {
                    r.rate_kind == "token"
                        && r.usage_class == usage_class
                        && r.context_tier.as_deref() == Some(tier.as_str())
                });
                if !has_tier_rate {
                    return Err(format!(
                        "missing token rate for usage_class={usage_class}, context_tier={tier}"
                    ));
                }
            }
        }
    }
    for usage_class in server_tool_usage_classes {
        let has_meter = resolution
            .rates
            .iter()
            .any(|r| r.rate_kind == "meter" && r.usage_class == *usage_class);
        if !has_meter {
            return Err(format!(
                "meter rate required for server-native tool usage class: {usage_class}"
            ));
        }
    }
    Ok(true)
}

fn determine_context_tier(
    usage: &urp::Usage,
    rates: &[DbBillingRateRecord],
) -> Result<Option<String>, String> {
    if let Some(tier) = usage
        .extra_body
        .get("context_tier")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Ok(Some(tier.to_string()));
    }

    let has_context_tiers = rates
        .iter()
        .any(|r| r.context_tier.as_deref().is_some_and(|v| v != "default"));
    if !has_context_tiers {
        return Ok(None);
    }

    let threshold = rates
        .iter()
        .filter_map(|r| r.match_json.get("context_threshold_tokens"))
        .find_map(parse_u64_value)
        .ok_or_else(|| "context-tier rate requires context_threshold_tokens".to_string())?;
    if usage.input_tokens > threshold {
        Ok(Some("long".to_string()))
    } else {
        Ok(Some("short".to_string()))
    }
}

fn usage_service_tier(usage: &urp::Usage) -> Option<String> {
    usage
        .extra_body
        .get("service_tier")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn rate_matches_dimension(
    rate: &DbBillingRateRecord,
    modality: Option<&str>,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
    cache_ttl: Option<&str>,
) -> bool {
    if let Some(rate_modality) = rate.modality.as_deref()
        && Some(rate_modality) != modality
    {
        return false;
    }
    if rate.modality.is_none() && modality.is_some() {
        return false;
    }
    if let Some(rate_context_tier) = rate.context_tier.as_deref()
        && Some(rate_context_tier) != context_tier
        && rate_context_tier != "default"
    {
        return false;
    }
    if let Some(rate_service_tier) = rate.service_tier.as_deref()
        && Some(rate_service_tier) != service_tier
        && rate_service_tier != "default"
    {
        return false;
    }
    if let Some(rate_cache_ttl) = rate.cache_ttl.as_deref()
        && Some(rate_cache_ttl) != cache_ttl
    {
        return false;
    }
    true
}

fn find_rate<'a>(
    rates: &'a [DbBillingRateRecord],
    rate_kind: &str,
    usage_class: &str,
    modality: Option<&str>,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
    cache_ttl: Option<&str>,
) -> Option<&'a DbBillingRateRecord> {
    rates.iter().find(|rate| {
        rate.rate_kind == rate_kind
            && rate.usage_class == usage_class
            && rate_matches_dimension(rate, modality, context_tier, service_tier, cache_ttl)
    })
}

fn find_rate_for_usage_classes<'a>(
    rates: &'a [DbBillingRateRecord],
    rate_kind: &str,
    usage_classes: &[&str],
    modality: Option<&str>,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
    cache_ttl: Option<&str>,
) -> Option<&'a DbBillingRateRecord> {
    usage_classes.iter().find_map(|usage_class| {
        find_rate(
            rates,
            rate_kind,
            usage_class,
            modality,
            context_tier,
            service_tier,
            cache_ttl,
        )
    })
}

fn has_modality_rates(rates: &[DbBillingRateRecord], usage_class: &str) -> bool {
    rates.iter().any(|rate| {
        rate.rate_kind == "token" && rate.usage_class == usage_class && rate.modality.is_some()
    })
}

fn has_any_modality_rates(rates: &[DbBillingRateRecord], usage_classes: &[&str]) -> bool {
    usage_classes
        .iter()
        .any(|usage_class| has_modality_rates(rates, usage_class))
}

fn modality_quantity_sum(breakdown: &urp::ModalityBreakdown) -> u64 {
    breakdown
        .text_tokens
        .unwrap_or(0)
        .saturating_add(breakdown.image_tokens.unwrap_or(0))
        .saturating_add(breakdown.audio_tokens.unwrap_or(0))
        .saturating_add(breakdown.video_tokens.unwrap_or(0))
        .saturating_add(breakdown.document_tokens.unwrap_or(0))
}

fn validate_modality_sum(
    usage_class: &str,
    breakdown: &urp::ModalityBreakdown,
    expected: u64,
) -> Result<(), String> {
    let actual = modality_quantity_sum(breakdown);
    if actual != expected {
        return Err(format!(
            "modality-specific rate for {usage_class} requires modality quantities to sum to billed quantity"
        ));
    }
    Ok(())
}

fn subtract_optional_modality(
    total: Option<u64>,
    subtract: Option<u64>,
    usage_class: &str,
) -> Result<Option<u64>, String> {
    match (total, subtract) {
        (Some(total), Some(subtract)) if subtract <= total => Ok(Some(total - subtract)),
        (Some(total), None) => Ok(Some(total)),
        (None, Some(0)) => Ok(None),
        (None, None) => Ok(None),
        _ => Err(format!(
            "modality-specific rate for {usage_class} requires compatible cache-read modality quantities"
        )),
    }
}

fn subtract_modality_breakdown(
    total: &urp::ModalityBreakdown,
    subtract: &urp::ModalityBreakdown,
    usage_class: &str,
) -> Result<urp::ModalityBreakdown, String> {
    Ok(urp::ModalityBreakdown {
        text_tokens: subtract_optional_modality(
            total.text_tokens,
            subtract.text_tokens,
            usage_class,
        )?,
        image_tokens: subtract_optional_modality(
            total.image_tokens,
            subtract.image_tokens,
            usage_class,
        )?,
        audio_tokens: subtract_optional_modality(
            total.audio_tokens,
            subtract.audio_tokens,
            usage_class,
        )?,
        video_tokens: subtract_optional_modality(
            total.video_tokens,
            subtract.video_tokens,
            usage_class,
        )?,
        document_tokens: subtract_optional_modality(
            total.document_tokens,
            subtract.document_tokens,
            usage_class,
        )?,
    })
}

fn input_uncached_modality_breakdown(
    details: Option<&urp::InputDetails>,
    uncached_tokens: u64,
) -> Result<Option<urp::ModalityBreakdown>, String> {
    let Some(details) = details else {
        return Ok(None);
    };
    let Some(total_breakdown) = details.modality_breakdown.as_ref() else {
        return Ok(None);
    };
    if details.cache_creation_tokens > 0 {
        return Err(
            "modality-specific input rate requires cache-creation modality quantities".to_string(),
        );
    }
    if details.cache_read_tokens == 0 {
        validate_modality_sum("input_uncached", total_breakdown, uncached_tokens)?;
        return Ok(Some(total_breakdown.clone()));
    }
    let cached_breakdown = details
        .cache_read_modality_breakdown
        .as_ref()
        .ok_or_else(|| {
            "modality-specific input rate requires cache-read modality quantities".to_string()
        })?;
    validate_modality_sum("cache_read", cached_breakdown, details.cache_read_tokens)?;
    let uncached_breakdown =
        subtract_modality_breakdown(total_breakdown, cached_breakdown, "input_uncached")?;
    validate_modality_sum("input_uncached", &uncached_breakdown, uncached_tokens)?;
    Ok(Some(uncached_breakdown))
}

fn add_token_line(
    line_items: &mut Vec<Value>,
    rates: &[DbBillingRateRecord],
    usage_class: &str,
    quantity: u64,
    modality: Option<&str>,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
    cache_ttl: Option<&str>,
) -> Result<i128, String> {
    add_token_line_for_usage_classes(
        line_items,
        rates,
        &[usage_class],
        quantity,
        modality,
        context_tier,
        service_tier,
        cache_ttl,
    )
}

fn add_token_line_for_usage_classes(
    line_items: &mut Vec<Value>,
    rates: &[DbBillingRateRecord],
    usage_classes: &[&str],
    quantity: u64,
    modality: Option<&str>,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
    cache_ttl: Option<&str>,
) -> Result<i128, String> {
    if quantity == 0 {
        return Ok(0);
    }
    let rate = find_rate_for_usage_classes(
        rates,
        "token",
        usage_classes,
        modality,
        context_tier,
        service_tier,
        cache_ttl,
    )
    .ok_or_else(|| {
        format!(
            "missing token rate for usage_class={}, modality={:?}, context_tier={:?}, cache_ttl={:?}",
            usage_classes.join("|"), modality, context_tier, cache_ttl
        )
    })?;
    let unit_price = rate.unit_price_nano()?;
    let charge = i128::from(quantity)
        .checked_mul(unit_price)
        .ok_or_else(|| "token charge overflow".to_string())?;
    line_items.push(json!({
        "rate_id": rate.id,
        "usage_class": rate.usage_class,
        "unit": rate.unit,
        "unit_price_nano": unit_price.to_string(),
        "quantity": quantity,
        "charge_nano": charge.to_string(),
        "modality": modality,
        "context_tier": context_tier,
        "service_tier": service_tier,
        "cache_ttl": cache_ttl,
    }));
    Ok(charge)
}

fn add_modality_token_lines(
    line_items: &mut Vec<Value>,
    rates: &[DbBillingRateRecord],
    usage_classes: &[&str],
    breakdown: Option<&urp::ModalityBreakdown>,
    fallback_quantity: u64,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
) -> Result<i128, String> {
    if !has_any_modality_rates(rates, usage_classes) {
        return add_token_line_for_usage_classes(
            line_items,
            rates,
            usage_classes,
            fallback_quantity,
            None,
            context_tier,
            service_tier,
            None,
        );
    }
    let Some(breakdown) = breakdown else {
        return Err(format!(
            "modality-specific rate for {} requires usage modality breakdown",
            usage_classes.join("|")
        ));
    };
    validate_modality_sum(usage_classes[0], breakdown, fallback_quantity)?;
    let mut total = 0i128;
    for (modality, quantity) in [
        ("text", breakdown.text_tokens),
        ("image", breakdown.image_tokens),
        ("audio", breakdown.audio_tokens),
        ("video", breakdown.video_tokens),
        ("document", breakdown.document_tokens),
    ] {
        total = total
            .checked_add(add_token_line_for_usage_classes(
                line_items,
                rates,
                usage_classes,
                quantity.unwrap_or(0),
                Some(modality),
                context_tier,
                service_tier,
                None,
            )?)
            .ok_or_else(|| "token charge overflow".to_string())?;
    }
    Ok(total)
}

fn authoritative_meter_quantity(usage: &urp::Usage, usage_class: &str, unit: &str) -> Option<u64> {
    let direct_keys = [
        usage_class.to_string(),
        format!("{usage_class}_requests"),
        format!("{usage_class}_calls"),
        format!("{usage_class}_billed_minutes"),
        format!("{usage_class}_minutes"),
    ];
    for key in &direct_keys {
        if let Some(value) = usage.extra_body.get(key).and_then(parse_u64_value) {
            return Some(value);
        }
    }
    if let Some(obj) = usage
        .extra_body
        .get("server_tool_use")
        .and_then(Value::as_object)
    {
        let key = match usage_class {
            "web_search" => "web_search_requests",
            "code_execution_duration" if unit == "billed_minute" => "code_execution_billed_minutes",
            "code_execution" => "code_execution_requests",
            _ => usage_class,
        };
        if let Some(value) = obj.get(key).and_then(parse_u64_value) {
            return Some(value);
        }
    }
    if let Some(obj) = usage
        .extra_body
        .get("server_side_tool_usage")
        .and_then(Value::as_object)
    {
        for key in [
            usage_class,
            &format!("{usage_class}_calls"),
            &format!("{usage_class}_requests"),
        ] {
            if let Some(value) = obj.get(key).and_then(parse_u64_value) {
                return Some(value);
            }
        }
    }
    None
}

fn decoded_provider_item_count(output: Option<&[urp::Node]>, usage_class: &str) -> u64 {
    let Some(output) = output else {
        return 0;
    };
    output
        .iter()
        .filter(|node| match node {
            urp::Node::ProviderItem { item_type, .. } => match usage_class {
                "web_search" => item_type.contains("web_search"),
                "file_search_tool_call" => item_type.contains("file_search"),
                "x_search" => item_type.contains("x_search"),
                "code_execution" | "code_execution_duration" | "code_interpreter_duration" => {
                    item_type.contains("code")
                }
                _ => false,
            },
            _ => false,
        })
        .count() as u64
}

fn add_meter_lines(
    line_items: &mut Vec<Value>,
    rates: &[DbBillingRateRecord],
    usage: &urp::Usage,
    output: Option<&[urp::Node]>,
    requested_usage_classes: &[String],
) -> Result<i128, String> {
    let mut total = 0i128;
    for rate in rates.iter().filter(|r| r.rate_kind == "meter") {
        let authoritative = authoritative_meter_quantity(usage, &rate.usage_class, &rate.unit);
        if requested_usage_classes
            .iter()
            .any(|usage_class| usage_class == &rate.usage_class)
            && rate
                .match_json
                .get("requires_authoritative_usage")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            && authoritative.is_none()
        {
            return Err(format!(
                "authoritative usage required for meter usage_class={}",
                rate.usage_class
            ));
        }
        let mut quantity = authoritative.unwrap_or_else(|| {
            if rate.unit == "call" {
                decoded_provider_item_count(output, &rate.usage_class)
            } else {
                0
            }
        });
        if quantity == 0 {
            continue;
        }
        if rate
            .match_json
            .get("requires_authoritative_usage")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && authoritative.is_none()
        {
            return Err(format!(
                "authoritative usage required for meter usage_class={}",
                rate.usage_class
            ));
        }
        if let Some(minimum) = rate
            .match_json
            .get("minimum_units")
            .and_then(parse_u64_value)
        {
            quantity = quantity.max(minimum);
        }
        let unit_price = rate.unit_price_nano()?;
        let charge = i128::from(quantity)
            .checked_mul(unit_price)
            .ok_or_else(|| "meter charge overflow".to_string())?;
        line_items.push(json!({
            "rate_id": rate.id,
            "usage_class": rate.usage_class,
            "unit": rate.unit,
            "unit_price_nano": unit_price.to_string(),
            "quantity": quantity,
            "charge_nano": charge.to_string(),
            "authoritative": authoritative.is_some(),
        }));
        total = total
            .checked_add(charge)
            .ok_or_else(|| "meter charge overflow".to_string())?;
    }
    Ok(total)
}

pub(super) fn calculate_rate_matrix_charge_components(
    usage: &urp::Usage,
    output: Option<&[urp::Node]>,
    resolution: &BillingRateResolution,
    provider_multiplier: f64,
    requested_usage_classes: &[String],
) -> Result<MatrixChargeComponents, String> {
    let input_details = usage.input_details.as_ref();
    let output_details = usage.output_details.as_ref();
    let context_tier = determine_context_tier(usage, &resolution.rates)?;
    let service_tier = usage_service_tier(usage);
    let context_tier_ref = context_tier.as_deref();
    let service_tier_ref = service_tier.as_deref();

    let cached_tokens = input_details.map(|d| d.cache_read_tokens).unwrap_or(0);
    let cache_creation_tokens = input_details.map(|d| d.cache_creation_tokens).unwrap_or(0);
    let cache_creation_5m = input_details
        .map(|d| d.cache_creation_5m_tokens)
        .unwrap_or(0);
    let cache_creation_1h = input_details
        .map(|d| d.cache_creation_1h_tokens)
        .unwrap_or(0);
    let uncached_tokens = usage
        .input_tokens
        .saturating_sub(cached_tokens)
        .saturating_sub(cache_creation_tokens);
    let reasoning_tokens = output_details.map(|d| d.reasoning_tokens).unwrap_or(0);
    let has_reasoning_rate = reasoning_tokens == 0
        || find_rate(
            &resolution.rates,
            "token",
            "reasoning_output",
            None,
            context_tier_ref,
            service_tier_ref,
            None,
        )
        .is_some();
    let non_reasoning_output_tokens = if has_reasoning_rate {
        usage.output_tokens.saturating_sub(reasoning_tokens)
    } else {
        usage.output_tokens
    };
    let billable_reasoning_tokens = if has_reasoning_rate {
        reasoning_tokens
    } else {
        0
    };
    let uncached_input_modality_breakdown =
        if has_any_modality_rates(&resolution.rates, &["input_uncached"]) {
            input_uncached_modality_breakdown(input_details, uncached_tokens)?
        } else {
            None
        };

    let mut token_line_items = Vec::new();
    let mut token_total = 0i128;
    token_total = token_total
        .checked_add(add_modality_token_lines(
            &mut token_line_items,
            &resolution.rates,
            &["input_uncached"],
            uncached_input_modality_breakdown.as_ref(),
            uncached_tokens,
            context_tier_ref,
            service_tier_ref,
        )?)
        .ok_or_else(|| "token charge overflow".to_string())?;
    token_total = token_total
        .checked_add(add_modality_token_lines(
            &mut token_line_items,
            &resolution.rates,
            &["cache_read", "input_cached"],
            input_details.and_then(|d| d.cache_read_modality_breakdown.as_ref()),
            cached_tokens,
            context_tier_ref,
            service_tier_ref,
        )?)
        .ok_or_else(|| "token charge overflow".to_string())?;

    let has_cache_5m_rate = find_rate(
        &resolution.rates,
        "token",
        "cache_write_5m",
        None,
        context_tier_ref,
        service_tier_ref,
        Some("5m"),
    )
    .is_some()
        || find_rate(
            &resolution.rates,
            "token",
            "cache_write_5m",
            None,
            context_tier_ref,
            service_tier_ref,
            None,
        )
        .is_some();
    let has_cache_1h_rate = find_rate(
        &resolution.rates,
        "token",
        "cache_write_1h",
        None,
        context_tier_ref,
        service_tier_ref,
        Some("1h"),
    )
    .is_some()
        || find_rate(
            &resolution.rates,
            "token",
            "cache_write_1h",
            None,
            context_tier_ref,
            service_tier_ref,
            None,
        )
        .is_some();
    if cache_creation_tokens > 0
        && cache_creation_5m == 0
        && cache_creation_1h == 0
        && has_cache_5m_rate
        && has_cache_1h_rate
    {
        return Err(
            "cache creation usage requires 5m/1h split for the selected rate matrix".to_string(),
        );
    }
    let aggregate_cache_write_5m = if cache_creation_5m == 0 && cache_creation_1h == 0 {
        cache_creation_tokens
    } else {
        cache_creation_5m
    };
    token_total = token_total
        .checked_add(add_token_line(
            &mut token_line_items,
            &resolution.rates,
            "cache_write_5m",
            aggregate_cache_write_5m,
            None,
            context_tier_ref,
            service_tier_ref,
            Some("5m"),
        )?)
        .ok_or_else(|| "token charge overflow".to_string())?;
    token_total = token_total
        .checked_add(add_token_line(
            &mut token_line_items,
            &resolution.rates,
            "cache_write_1h",
            cache_creation_1h,
            None,
            context_tier_ref,
            service_tier_ref,
            Some("1h"),
        )?)
        .ok_or_else(|| "token charge overflow".to_string())?;
    token_total = token_total
        .checked_add(add_modality_token_lines(
            &mut token_line_items,
            &resolution.rates,
            &["output"],
            output_details.and_then(|d| d.modality_breakdown.as_ref()),
            non_reasoning_output_tokens,
            context_tier_ref,
            service_tier_ref,
        )?)
        .ok_or_else(|| "token charge overflow".to_string())?;
    token_total = token_total
        .checked_add(add_token_line(
            &mut token_line_items,
            &resolution.rates,
            "reasoning_output",
            billable_reasoning_tokens,
            None,
            context_tier_ref,
            service_tier_ref,
            None,
        )?)
        .ok_or_else(|| "token charge overflow".to_string())?;

    let mut meter_line_items = Vec::new();
    let meter_total = add_meter_lines(
        &mut meter_line_items,
        &resolution.rates,
        usage,
        output,
        requested_usage_classes,
    )?;
    let base_charge = token_total
        .checked_add(meter_total)
        .ok_or_else(|| "charge overflow".to_string())?;
    let final_charge = scale_charge_with_multiplier(base_charge, provider_multiplier)
        .ok_or_else(|| "charge overflow".to_string())?;

    Ok(MatrixChargeComponents {
        token_line_items,
        meter_line_items,
        context_tier,
        service_tier,
        base_charge,
        final_charge,
    })
}

fn build_matrix_billing_breakdown(
    logical_model: &str,
    attempt: &MonoizeAttempt,
    resolution: &BillingRateResolution,
    components: &MatrixChargeComponents,
) -> Value {
    json!({
        "version": 2,
        "currency": "nano_usd",
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "pricing_model": resolution.pricing_model,
        "pricing_profile": resolution.pricing_profile,
        "provider_id": attempt.provider_id,
        "provider_multiplier": attempt.model_multiplier,
        "tier": {
            "context_tier": components.context_tier,
            "service_tier": components.service_tier,
        },
        "token_line_items": components.token_line_items,
        "meter_line_items": components.meter_line_items,
        "base_charge_nano": components.base_charge.to_string(),
        "final_charge_nano": components.final_charge.to_string(),
    })
}

pub(super) fn scale_charge_with_multiplier(
    base_nano: i128,
    provider_multiplier: f64,
) -> Option<i128> {
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
    maybe_charge_usage_with_output(state, auth, attempt, logical_model, usage, None).await
}

async fn maybe_charge_usage_with_output(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: &urp::Usage,
    output: Option<&[urp::Node]>,
) -> AppResult<ChargeComputation> {
    let resolution = match resolve_billing_rate_matrix(
        state,
        &attempt.upstream_model,
        logical_model,
        attempt.provider_type,
    )
    .await?
    {
        Some(v) => v,
        None => {
            return Err(AppError::new(
                StatusCode::FORBIDDEN,
                "model_pricing_required",
                format!(
                    "pricing metadata required for model: {}",
                    attempt.upstream_model
                ),
            ));
        }
    };
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(ChargeComputation::default());
    };

    let components = match calculate_rate_matrix_charge_components(
        usage,
        output,
        &resolution,
        attempt.model_multiplier,
        &attempt.server_tool_usage_classes,
    ) {
        Ok(v) => v,
        Err(err) => {
            if err.contains("missing token rate")
                || err.contains("requires")
                || err.contains("authoritative usage required")
            {
                return Err(AppError::new(
                    StatusCode::FORBIDDEN,
                    "model_pricing_required",
                    err,
                ));
            }
            tracing::error!(
                "billing error: charge calculation failed for model={}: {}",
                attempt.upstream_model,
                err
            );
            return Err(AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "billing_overflow",
                err,
            ));
        }
    };
    if !attempt.model_multiplier.is_finite() || attempt.model_multiplier < 0.0 {
        tracing::error!(
            "billing error: charge overflow for model={}",
            attempt.upstream_model
        );
        return Err(AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "billing_overflow",
            format!(
                "charge calculation overflow for model={}",
                attempt.upstream_model
            ),
        ));
    }
    let billing_breakdown =
        build_matrix_billing_breakdown(logical_model, attempt, &resolution, &components);
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
        "cache_creation_5m_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_5m_tokens),
        "cache_creation_1h_tokens": usage.input_details.as_ref().map(|d| d.cache_creation_1h_tokens),
        "reasoning_tokens": usage.reasoning_tokens(),
        "charge_nano_usd": charge_nano.to_string(),
        "api_key_id": auth.api_key_id,
    });

    if auth.sub_account_enabled {
        let api_key_id = auth.api_key_id.as_deref().unwrap_or("");
        match state
            .user_store
            .charge_sub_account_balance_nano(api_key_id, user_id, charge_nano, &meta)
            .await
        {
            Ok(()) => {
                return Ok(ChargeComputation {
                    charge_nano_usd: Some(charge_nano),
                    billing_breakdown: Some(billing_breakdown),
                });
            }
            Err(err) => match err.kind {
                BillingErrorKind::InsufficientBalance => {
                    return Err(AppError::new(
                        StatusCode::PAYMENT_REQUIRED,
                        "insufficient_balance",
                        "insufficient balance",
                    ));
                }
                BillingErrorKind::NotFound => {
                    return Err(AppError::new(
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                        "api key not found",
                    ));
                }
                _ => {
                    return Err(AppError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal_error",
                        err.message,
                    ));
                }
            },
        }
    }

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
    maybe_charge_usage_with_output(
        state,
        auth,
        attempt,
        logical_model,
        usage,
        Some(response.output.as_slice()),
    )
    .await
}

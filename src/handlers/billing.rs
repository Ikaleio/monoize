use super::*;
use crate::model_price_store::ModelPriceRecord;
use crate::settlement::{self, SettledUsage, SettlementInputs};

/// Result of one settlement attempt: the amount charged to the ledger (when
/// positive) plus the version-3 breakdown persisted with the request log.
#[derive(Debug, Clone, Default)]
pub(super) struct ChargeComputation {
    pub(super) charge_nano_usd: Option<i128>,
    pub(super) billing_breakdown: Option<Value>,
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

/// MP-R1/MP-R8 preflight snapshot: the applicable `model_prices` rows for
/// every distinct pricing key of one forwarding request, loaded by one
/// set-based query.
#[derive(Debug, Clone, Default)]
pub(super) struct ModelPriceSnapshot {
    reasoning_suffix_map: HashMap<String, String>,
    rows: HashMap<String, ModelPriceRecord>,
}

impl ModelPriceSnapshot {
    /// MP-R1: try the normalized upstream key; on a miss for a redirected
    /// model, retry the normalized logical key. Returns the pricing key that
    /// the breakdown records plus the applicable row when one exists.
    pub(super) fn resolve(
        &self,
        upstream_model: &str,
        logical_model: &str,
    ) -> (String, Option<ModelPriceRecord>) {
        let upstream_key = normalize_pricing_model_key(upstream_model, &self.reasoning_suffix_map);
        if let Some(row) = self.rows.get(&upstream_key) {
            return (upstream_key, Some(row.clone()));
        }
        let logical_key = normalize_pricing_model_key(logical_model, &self.reasoning_suffix_map);
        if logical_key != upstream_key
            && let Some(row) = self.rows.get(&logical_key)
        {
            return (logical_key, Some(row.clone()));
        }
        (upstream_key, None)
    }
}

pub(super) async fn build_model_price_snapshot(
    state: &AppState,
    upstream_models: &[String],
    logical_model: &str,
) -> AppResult<ModelPriceSnapshot> {
    let reasoning_suffix_map = state
        .monoize_runtime
        .read()
        .await
        .reasoning_suffix_map
        .clone();
    let mut keys: Vec<String> = upstream_models
        .iter()
        .map(|model| normalize_pricing_model_key(model, &reasoning_suffix_map))
        .collect();
    keys.push(normalize_pricing_model_key(
        logical_model,
        &reasoning_suffix_map,
    ));
    keys.sort();
    keys.dedup();
    let rows = state
        .model_price_store
        .list_by_model_ids(&keys)
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let rows = rows
        .into_iter()
        // MP-R2/MP-R4: a disabled or incomplete row is exactly a missing row.
        .filter(|row| row.enabled && row.is_complete())
        .map(|row| (row.model_id.clone(), row))
        .collect();
    Ok(ModelPriceSnapshot {
        reasoning_suffix_map,
        rows,
    })
}

/// MP-F3/MP-F5: `true` when a billable success without normalized usage must
/// reject (non-stream and buffered synthetic stream reject with 403
/// `usage_required`; pass-through streams settle from the byte estimate
/// instead). An unpriced attempt settles free under MP-F5 and never rejects
/// for missing usage.
pub(super) fn missing_usage_rejects(
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
) -> bool {
    auth.user_id.is_some()
        && attempt.model_price.is_some()
        && !attempt.allow_free_when_missing_usage
}

pub(super) fn missing_usage_error() -> AppError {
    AppError::new(
        StatusCode::FORBIDDEN,
        "usage_required",
        "upstream response did not include billable usage",
    )
}

/// Settle one attempt (MP-C11, MP-B1) and charge the ledger when positive.
pub(super) async fn maybe_charge_settled(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: SettledUsage<'_>,
    output: Option<&[urp::Node]>,
    response_service_tier: Option<&str>,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    let Some(user_id) = auth.user_id.as_deref() else {
        return Ok(ChargeComputation::default());
    };
    let tool_prices = state.monoize_runtime.read().await.tool_prices.clone();
    let inputs = SettlementInputs {
        usage,
        output,
        price: attempt.model_price.as_ref(),
        pricing_model_key: &attempt.pricing_model_key,
        tool_prices: &tool_prices,
        requested_tool_classes: &attempt.server_tool_usage_classes,
        service_tier: response_service_tier
            .map(str::trim)
            .filter(|tier| !tier.is_empty()),
        billing_group_id: attempt.billing_group_id.as_deref(),
        group_billing_ratio: attempt.group_billing_ratio,
        channel_multiplier: attempt.model_multiplier,
    };
    let outcome = settlement::settle(&inputs).map_err(|err| {
        tracing::error!(
            "billing error: settlement failed for model={}: {}",
            attempt.upstream_model,
            err
        );
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "billing_settlement_failed",
            err,
        )
    })?;
    let charge_nano = outcome.final_charge_nano;
    let billing_breakdown = outcome.breakdown;
    if charge_nano <= 0 {
        return Ok(ChargeComputation {
            charge_nano_usd: None,
            billing_breakdown: Some(billing_breakdown),
        });
    }

    let reported_usage = usage.usage();
    let meta = json!({
        "logical_model": logical_model,
        "upstream_model": attempt.upstream_model,
        "provider_id": attempt.provider_id,
        "channel_multiplier": attempt.model_multiplier,
        "group_billing_ratio": attempt.group_billing_ratio,
        "billing_group_id": attempt.billing_group_id,
        "prompt_tokens": reported_usage.map(|u| u.input_tokens),
        "completion_tokens": reported_usage.map(|u| u.output_tokens),
        "cached_tokens": reported_usage.and_then(|u| u.cached_tokens()),
        "cache_creation_tokens": reported_usage
            .and_then(|u| u.input_details.as_ref())
            .map(|d| d.cache_creation_tokens),
        "reasoning_tokens": reported_usage.and_then(|u| u.reasoning_tokens()),
        "charge_nano_usd": charge_nano.to_string(),
        "api_key_id": auth.api_key_id,
        "request_id": request_id,
    });

    if state.node.is_replica() {
        // M3: on replicas the synchronous charge path is replaced by a durable
        // balance-delta enqueue; the primary applies it idempotently later.
        if auth.sub_account_enabled {
            crate::replica::metering::ReplicaMetering::enqueue_balance_delta_for_request(
                state,
                "api_key_charge",
                user_id,
                auth.api_key_id.as_deref(),
                charge_nano,
                &meta,
            )
            .await
            .map_err(|err| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "metering_enqueue_failed",
                    err,
                )
            })?;
        } else {
            crate::replica::metering::ReplicaMetering::enqueue_balance_delta_for_request(
                state,
                "request_charge",
                user_id,
                None,
                charge_nano,
                &meta,
            )
            .await
            .map_err(|err| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "metering_enqueue_failed",
                    err,
                )
            })?;
        }
        return Ok(ChargeComputation {
            charge_nano_usd: Some(charge_nano),
            billing_breakdown: Some(billing_breakdown),
        });
    }

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

/// Settle a terminal pass-through stream.
#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_charge_stream_usage(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    usage: SettledUsage<'_>,
    output: &[urp::Node],
    response_service_tier: Option<&str>,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    maybe_charge_settled(
        state,
        auth,
        attempt,
        logical_model,
        usage,
        Some(output),
        response_service_tier,
        request_id,
    )
    .await
}

/// Settle a terminal non-stream/buffered response. Callers enforce the
/// MP-F3 fail-closed rejection (`missing_usage_rejects`) before delivery;
/// a missing-usage response that reaches this point settles free.
pub(super) async fn maybe_charge_response(
    state: &AppState,
    auth: &crate::auth::AuthResult,
    attempt: &MonoizeAttempt,
    logical_model: &str,
    response: &urp::UrpResponse,
    request_id: Option<&str>,
) -> AppResult<ChargeComputation> {
    let response_service_tier = response
        .extra_body
        .get("service_tier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|tier| !tier.is_empty());
    let usage = match response.usage.as_ref() {
        Some(usage) => SettledUsage::Reported(usage),
        None => SettledUsage::MissingFree,
    };
    maybe_charge_settled(
        state,
        auth,
        attempt,
        logical_model,
        usage,
        Some(response.output.as_slice()),
        response_service_tier,
        request_id,
    )
    .await
}

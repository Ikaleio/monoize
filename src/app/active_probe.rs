use super::*;

const ACTIVE_PROBE_CONNECTIVITY_KIND: &str = "active_probe_connectivity";
const ACTIVE_PROBE_SYSTEM_USER: &str = "_monoize_active_probe";
pub(super) async fn ensure_active_probe_system_user(user_store: &UserStore) -> AppResult<String> {
    let existing = user_store
        .get_user_by_username(ACTIVE_PROBE_SYSTEM_USER)
        .await
        .map_err(active_probe_user_init_error)?;
    if let Some(user) = existing {
        if !user.balance_unlimited {
            user_store
                .update_user(
                    &user.id,
                    crate::users::UpdateUserInput {
                        balance_unlimited: Some(true),
                        ..Default::default()
                    },
                )
                .await
                .map_err(active_probe_user_init_error)?;
        }
        return Ok(user.id);
    }
    let user = user_store
        .create_user(
            ACTIVE_PROBE_SYSTEM_USER,
            &uuid::Uuid::new_v4().to_string(),
            UserRole::User,
            None,
        )
        .await
        .map_err(active_probe_user_init_error)?;
    user_store
        .update_user(
            &user.id,
            crate::users::UpdateUserInput {
                balance_unlimited: Some(true),
                ..Default::default()
            },
        )
        .await
        .map_err(active_probe_user_init_error)?;
    Ok(user.id)
}

fn active_probe_user_init_error(error: String) -> AppError {
    AppError::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "active_probe_user_init_failed",
        error,
    )
}

/// MP-M7 (`model-pricing.spec.md`): the active probe prices through
/// `model_prices` with MP-R1 key normalization. Applicable rows for every
/// candidate pricing key are loaded by one set-based query (MP-R8).
#[derive(Debug, Clone, Default)]
pub(super) struct ActiveProbePricingSnapshot {
    reasoning_suffix_map: HashMap<String, String>,
    rows: HashMap<String, crate::model_price_store::ModelPriceRecord>,
}

impl ActiveProbePricingSnapshot {
    /// MP-R1: normalized upstream key first, then the normalized logical key
    /// for redirected models. Returns the pricing key plus the applicable row.
    pub(super) fn resolve(
        &self,
        upstream_model: &str,
        logical_model: &str,
    ) -> (String, Option<crate::model_price_store::ModelPriceRecord>) {
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

pub(super) async fn build_active_probe_pricing_snapshot(
    model_price_store: &crate::model_price_store::ModelPriceStore,
    providers: &[crate::monoize_routing::MonoizeProvider],
    runtime: &MonoizeRuntimeConfig,
) -> Result<ActiveProbePricingSnapshot, String> {
    let reasoning_suffix_map = runtime.reasoning_suffix_map.clone();
    let mut keys = std::collections::HashSet::new();
    for provider in providers {
        for channel in &provider.channels {
            let configured_model = channel
                .active_probe_model_override
                .as_ref()
                .or(provider.active_probe_model_override.as_ref())
                .or(runtime.active_probe_model.as_ref())
                .cloned();
            let Some(logical_model) =
                configured_model.or_else(|| channel.models.keys().min().cloned())
            else {
                continue;
            };
            let Some(entry) = channel.models.get(&logical_model) else {
                continue;
            };
            let upstream_model = entry
                .redirect
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&logical_model);
            keys.insert(normalize_pricing_model_key(
                upstream_model,
                &reasoning_suffix_map,
            ));
            keys.insert(normalize_pricing_model_key(
                &logical_model,
                &reasoning_suffix_map,
            ));
        }
    }
    let mut keys = keys.into_iter().collect::<Vec<_>>();
    keys.sort();
    let rows = model_price_store.list_by_model_ids(&keys).await?;
    let rows = rows
        .into_iter()
        // MP-R2/MP-R4: a disabled or incomplete row is exactly a missing row.
        .filter(|row| row.enabled && row.is_complete())
        .map(|row| (row.model_id.clone(), row))
        .collect();
    Ok(ActiveProbePricingSnapshot {
        reasoning_suffix_map,
        rows,
    })
}

fn build_probe_usage_breakdown(prompt_tokens: u64, completion_tokens: u64) -> Value {
    json!({
        "version": 1,
        "input": {
            "total_tokens": prompt_tokens,
            "uncached_tokens": prompt_tokens,
            "text_tokens": prompt_tokens,
            "cached_tokens": 0,
            "cache_creation_tokens": null,
            "audio_tokens": null,
            "image_tokens": null
        },
        "output": {
            "total_tokens": completion_tokens,
            "non_reasoning_tokens": completion_tokens,
            "text_tokens": completion_tokens,
            "reasoning_tokens": null,
            "audio_tokens": null,
            "image_tokens": null
        },
        "raw_usage_extra": {
            "source": "active_probe"
        }
    })
}

#[derive(Clone, Copy)]
pub(super) struct ActiveProbeLogContext<'a> {
    pub(super) request_id: &'a str,
    pub(super) user_id: &'a str,
    pub(super) provider_id: &'a str,
    pub(super) provider_name: &'a str,
    pub(super) provider_type: crate::monoize_routing::MonoizeProviderType,
    pub(super) channel_multiplier: Option<Multiplier>,
    pub(super) channel_id: &'a str,
    pub(super) channel_name: &'a str,
    pub(super) logical_model: &'a str,
    pub(super) upstream_model: &'a str,
    pub(super) created_at: chrono::DateTime<chrono::Utc>,
}

pub(super) fn build_active_probe_interrupted_log(
    context: ActiveProbeLogContext<'_>,
) -> InsertRequestLog {
    let ActiveProbeLogContext {
        request_id,
        user_id,
        provider_id,
        provider_name,
        provider_type,
        channel_multiplier,
        channel_id,
        channel_name,
        logical_model,
        upstream_model,
        created_at,
    } = context;
    InsertRequestLog {
        request_id: Some(request_id.to_string()),
        user_id: user_id.to_string(),
        api_key_id: None,
        model: logical_model.to_string(),
        provider_id: Some(provider_id.to_string()),
        upstream_model: Some(upstream_model.to_string()),
        upstream_response_model: None,
        channel_id: Some(channel_id.to_string()),
        names: crate::users::RequestLogNameSnapshots {
            username: Some(ACTIVE_PROBE_SYSTEM_USER.to_string()),
            api_key_name: None,
            provider_name: Some(provider_name.to_string()),
            channel_name: Some(channel_name.to_string()),
        },
        is_stream: false,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: Some(channel_multiplier.unwrap_or(Multiplier::ONE)),
        charge_nano_usd: None,
        status: crate::users::REQUEST_LOG_STATUS_ERROR.to_string(),
        usage_breakdown_json: None,
        billing_breakdown_json: None,
        error_code: Some("active_probe_interrupted".to_string()),
        error_message: Some(
            "active probe ended before terminal request-log persistence".to_string(),
        ),
        error_http_status: Some(axum::http::StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
        duration_ms: None,
        ttfb_ms: None,
        request_ip: None,
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: Some(ACTIVE_PROBE_CONNECTIVITY_KIND.to_string()),
        effective_provider_type: Some(provider_type.as_str().to_string()),
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at,
    }
}

pub(super) async fn persist_active_probe_request_log(
    user_store: &UserStore,
    context: ActiveProbeLogContext<'_>,
    pricing: (String, Option<crate::model_price_store::ModelPriceRecord>),
    usage_snapshot: Option<Value>,
    duration_ms: u64,
    reservation: crate::db_cache::RequestLogReservation,
) -> Result<(), String> {
    let ActiveProbeLogContext {
        request_id,
        user_id,
        provider_id,
        provider_name,
        provider_type,
        channel_multiplier,
        channel_id,
        channel_name,
        logical_model,
        upstream_model,
        created_at,
    } = context;
    let channel_multiplier = channel_multiplier.unwrap_or(Multiplier::ONE);
    let (usage_tokens, charge_nano_usd, billing_breakdown_json) =
        settle_active_probe(context, pricing, usage_snapshot);

    let usage_breakdown_json = usage_tokens.map(|(prompt_tokens, completion_tokens)| {
        build_probe_usage_breakdown(prompt_tokens, completion_tokens)
    });

    let log = InsertRequestLog {
        request_id: Some(request_id.to_string()),
        user_id: user_id.to_string(),
        api_key_id: None,
        model: logical_model.to_string(),
        provider_id: Some(provider_id.to_string()),
        upstream_model: Some(upstream_model.to_string()),
        upstream_response_model: None,
        channel_id: Some(channel_id.to_string()),
        names: crate::users::RequestLogNameSnapshots {
            username: Some(ACTIVE_PROBE_SYSTEM_USER.to_string()),
            api_key_name: None,
            provider_name: Some(provider_name.to_string()),
            channel_name: Some(channel_name.to_string()),
        },
        is_stream: false,
        input_tokens: usage_tokens.map(|(prompt_tokens, _)| prompt_tokens),
        output_tokens: usage_tokens.map(|(_, completion_tokens)| completion_tokens),
        cache_read_tokens: usage_tokens.map(|_| 0),
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: Some(channel_multiplier),
        charge_nano_usd,
        status: "success".to_string(),
        usage_breakdown_json,
        billing_breakdown_json,
        error_code: None,
        error_message: None,
        error_http_status: None,
        duration_ms: Some(duration_ms),
        ttfb_ms: None,
        request_ip: None,
        reasoning_effort: None,
        tried_providers_json: None,
        request_kind: Some(ACTIVE_PROBE_CONNECTIVITY_KIND.to_string()),
        effective_provider_type: Some(provider_type.as_str().to_string()),
        affinity_hit: None,
        affinity_key_hash: None,
        affinity_target: None,
        session_affinity_value: None,
        created_at,
    };

    user_store
        .finalize_reserved_request_log(log, reservation)
        .await
}

pub(super) fn clear_channel_health_state(state: &mut ChannelHealthState, now: i64) {
    state.healthy = true;
    state.cooldown_until = None;
    state.last_success_at = Some(now);
    state.probe_success_count = 0;
    state.last_probe_at = None;
}

pub(super) mod scheduler;

fn settle_active_probe(
    context: ActiveProbeLogContext<'_>,
    pricing: (String, Option<crate::model_price_store::ModelPriceRecord>),
    usage_snapshot: Option<Value>,
) -> (Option<(u64, u64)>, Option<i128>, Option<Value>) {
    let ActiveProbeLogContext {
        logical_model,
        upstream_model,
        channel_multiplier,
        ..
    } = context;
    let channel_multiplier = channel_multiplier.unwrap_or(Multiplier::ONE);
    let (pricing_model_key, model_price) = pricing;
    let parsed_prompt_tokens = usage_snapshot
        .as_ref()
        .and_then(|v| v.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let parsed_completion_tokens = usage_snapshot
        .as_ref()
        .and_then(|v| v.get("completion_tokens"))
        .and_then(|v| v.as_u64());
    let usage_tokens = parsed_prompt_tokens.zip(parsed_completion_tokens);
    let (charge_nano_usd, billing_breakdown_json) = if let Some(price) = model_price.as_ref()
        && let Some((prompt_tokens, completion_tokens)) = usage_tokens
    {
        let usage = crate::urp::Usage {
            iterations: None,
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
            input_details: None,
            output_details: None,
            extra_body: HashMap::new(),
        };
        // The probe settles at group ratio 1 and the channel multiplier; the
        // probe user has no group memberships.
        let inputs = crate::settlement::SettlementInputs {
            usage: crate::settlement::SettledUsage::Reported(&usage),
            output: None,
            price: Some(price),
            pricing_model_key: &pricing_model_key,
            tool_prices: &Value::Object(serde_json::Map::new()),
            requested_tool_classes: &[],
            service_tier: None,
            billing_group_id: None,
            group_billing_ratio: Multiplier::ONE,
            channel_multiplier,
        };
        match crate::settlement::settle(&inputs) {
            Ok(outcome) => (Some(outcome.final_charge_nano), Some(outcome.breakdown)),
            Err(err) => {
                tracing::warn!(
                    logical_model,
                    upstream_model,
                    error = %err,
                    "active probe charge calculation failed"
                );
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    (usage_tokens, charge_nano_usd, billing_breakdown_json)
}

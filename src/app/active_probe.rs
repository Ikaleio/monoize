use super::*;

pub(super) mod scheduler;

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
struct ActiveProbePricingSnapshot {
    reasoning_suffix_map: HashMap<String, String>,
    rows: HashMap<String, crate::model_price_store::ModelPriceRecord>,
}

impl ActiveProbePricingSnapshot {
    /// MP-R1: normalized upstream key first, then the normalized logical key
    /// for redirected models. Returns the pricing key plus the applicable row.
    fn resolve(
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

async fn build_active_probe_pricing_snapshot<'a>(
    model_price_store: &crate::model_price_store::ModelPriceStore,
    probes: impl IntoIterator<Item = crate::monoize_routing::ChannelProbe<'a>>,
    reasoning_suffix_map: HashMap<String, String>,
) -> Result<ActiveProbePricingSnapshot, String> {
    let mut keys = std::collections::HashSet::new();
    for probe in probes {
        for model in [probe.upstream_model, probe.logical_model] {
            keys.insert(normalize_pricing_model_key(model, &reasoning_suffix_map));
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

/// Identity of one probe attempt, shared by its armed fallback log and its success log.
#[derive(Clone, Copy)]
struct ActiveProbeLogContext<'a> {
    request_id: &'a str,
    user_id: &'a str,
    provider_id: &'a str,
    provider_name: &'a str,
    provider_type: crate::monoize_routing::MonoizeProviderType,
    channel_multiplier: Option<Multiplier>,
    channel_id: &'a str,
    channel_name: &'a str,
    logical_model: &'a str,
    upstream_model: &'a str,
    is_stream: bool,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn build_active_probe_interrupted_log(context: ActiveProbeLogContext<'_>) -> InsertRequestLog {
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
        is_stream,
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
        is_stream,
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

async fn persist_active_probe_request_log(
    user_store: &UserStore,
    context: ActiveProbeLogContext<'_>,
    pricing: (String, Option<crate::model_price_store::ModelPriceRecord>),
    usage_snapshot: Option<crate::urp::Usage>,
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
        is_stream,
        created_at,
    } = context;
    let channel_multiplier = channel_multiplier.unwrap_or(Multiplier::ONE);
    let (pricing_model_key, model_price) = pricing;
    let usage_snapshot = usage_snapshot.filter(|usage| usage.reported_token_counts);
    let (charge_nano_usd, billing_breakdown_json) = if let Some(price) = model_price.as_ref()
        && let Some(usage) = usage_snapshot.as_ref()
    {
        // The probe settles at group ratio 1 and the channel multiplier; the
        // probe user has no group memberships.
        let inputs = crate::settlement::SettlementInputs {
            usage: crate::settlement::SettledUsage::Reported(usage),
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

    let mut log = InsertRequestLog {
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
        is_stream,
        input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        tool_prompt_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        provider_multiplier: Some(channel_multiplier),
        charge_nano_usd,
        status: "success".to_string(),
        usage_breakdown_json: None,
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

    crate::handlers::apply_usage_fields(&mut log, usage_snapshot.as_ref());
    user_store
        .finalize_reserved_request_log(log, reservation)
        .await
}

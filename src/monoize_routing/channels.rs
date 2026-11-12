use super::store::MonoizeRoutingStore;
use super::types::*;
use super::validation::*;
use super::values::*;
use crate::urp::encode::openai_chat::ChatDialect;
use chrono::Utc;
use sea_orm::{ConnectionTrait, Value as SeaValue};
use std::collections::HashMap;

impl MonoizeRoutingStore {
    /// CM-CH-EN-1: flips one Channel's `enabled` flag without touching any other field.
    pub async fn set_channel_enabled(
        &self,
        provider_id: &str,
        channel_id: &str,
        enabled: bool,
    ) -> Result<bool, String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "UPDATE monoize_channels SET enabled = $1, updated_at = $2
                 WHERE id = $3 AND provider_id = $4",
                vec![
                    SeaValue::Int(Some(i32::from(enabled))),
                    Utc::now().to_rfc3339().into(),
                    channel_id.into(),
                    provider_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        self.catalog.invalidate();
        Ok(true)
    }

    pub(super) async fn replace_channels_on(
        &self,
        conn: &impl ConnectionTrait,
        provider_id: &str,
        channels: &[CreateMonoizeChannelInput],
    ) -> Result<(), String> {
        let existing_rows = conn
            .query_all(self.db.stmt(
                "SELECT id, api_key
                 FROM monoize_channels
                 WHERE provider_id = $1",
                vec![provider_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut existing_channels: HashMap<String, ExistingChannel> = HashMap::new();
        for row in &existing_rows {
            let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            existing_channels.insert(
                id,
                ExistingChannel {
                    api_key: row.try_get("", "api_key").map_err(|e| e.to_string())?,
                },
            );
        }

        conn.execute(self.db.stmt(
            "DELETE FROM monoize_channels WHERE provider_id = $1",
            vec![provider_id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let prepared = prepare_channels(channels, &existing_channels)?;
        let now = Utc::now().to_rfc3339();
        self.insert_channels_on(conn, provider_id, &prepared, &now)
            .await?;
        self.insert_channel_models_on(conn, &prepared, &now).await?;
        Ok(())
    }

    async fn insert_channels_on(
        &self,
        conn: &impl ConnectionTrait,
        provider_id: &str,
        prepared: &[PreparedChannel<'_>],
        now: &str,
    ) -> Result<(), String> {
        const CHANNEL_INSERT_CHUNK_SIZE: usize = 18;
        for chunk in prepared.chunks(CHANNEL_INSERT_CHUNK_SIZE) {
            let mut values: Vec<SeaValue> = Vec::with_capacity(chunk.len() * 27);
            let mut rows = Vec::with_capacity(chunk.len());
            for channel in chunk {
                let start = values.len() + 1;
                let input = channel.input;
                values.extend([
                    channel.id.clone().into(),
                    provider_id.into(),
                    input.name.as_str().into(),
                    input.provider_type.as_str().into(),
                    input.base_url.as_str().into(),
                    channel.api_key.clone().into(),
                    SeaValue::Int(Some(input.weight)),
                    SeaValue::Int(Some(if input.enabled { 1 } else { 0 })),
                    opt_u64_to_value(
                        input
                            .passive_failure_count_threshold_override
                            .map(|value| value as u64),
                    ),
                    opt_u64_to_value(input.passive_cooldown_seconds_override),
                    opt_u64_to_value(input.passive_window_seconds_override),
                    opt_u64_to_value(input.passive_rate_limit_cooldown_seconds_override),
                    opt_bool_to_value(input.active_probe_enabled_override),
                    opt_u64_to_value(input.active_probe_interval_seconds_override),
                    opt_u64_to_value(
                        input
                            .active_probe_success_threshold_override
                            .map(|value| value as u64),
                    ),
                    input.active_probe_model_override.clone().into(),
                    opt_bool_to_value(input.affinity_enabled_override),
                    opt_u64_to_value(input.affinity_idle_ttl_seconds_override),
                    input
                        .affinity_failback_mode_override
                        .map(|mode| mode.as_str().to_string())
                        .into(),
                    opt_u64_to_value(input.affinity_failback_delay_seconds_override),
                    normalized_proxy_url(input.proxy_url.as_deref()).into(),
                    normalized_extra_headers_json(input.extra_headers.as_ref()).into(),
                    opt_bool_to_value(input.session_affinity_auto),
                    opt_bool_to_value(input.websocket_supported),
                    channel
                        .chat_dialect
                        .map(|dialect| dialect.as_str().to_string())
                        .into(),
                    now.into(),
                    now.into(),
                ]);
                rows.push(format!(
                    "({})",
                    (start..start + 27)
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            conn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO monoize_channels
                     (id, provider_id, name, provider_type, base_url, api_key, weight, enabled,
                      passive_failure_count_threshold_override, passive_cooldown_seconds_override,
                      passive_window_seconds_override, passive_rate_limit_cooldown_seconds_override,
                      active_probe_enabled_override, active_probe_interval_seconds_override,
                      active_probe_success_threshold_override, active_probe_model_override,
                      affinity_enabled_override, affinity_idle_ttl_seconds_override,
                      affinity_failback_mode_override, affinity_failback_delay_seconds_override,
                      proxy_url,
                      extra_headers,
                      session_affinity_auto,
                      websocket_supported,
                      chat_dialect,
                      created_at, updated_at)
                     VALUES {}",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn insert_channel_models_on(
        &self,
        conn: &impl ConnectionTrait,
        prepared: &[PreparedChannel<'_>],
        now: &str,
    ) -> Result<(), String> {
        let model_rows = prepared
            .iter()
            .flat_map(|channel| {
                channel
                    .models
                    .iter()
                    .map(|(model, entry)| (channel.id.clone(), model.clone(), entry.clone()))
            })
            .collect::<Vec<_>>();
        const MODEL_INSERT_CHUNK_SIZE: usize = 66;
        for chunk in model_rows.chunks(MODEL_INSERT_CHUNK_SIZE) {
            let mut values: Vec<SeaValue> = Vec::with_capacity(chunk.len() * 6);
            let mut rows = Vec::with_capacity(chunk.len());
            for (channel_id, model, entry) in chunk {
                let start = values.len() + 1;
                values.extend([
                    format!("mono_ch_model_{}", uuid::Uuid::new_v4().simple()).into(),
                    channel_id.clone().into(),
                    model.clone().into(),
                    entry.redirect.clone().into(),
                    entry.multiplier.to_string().into(),
                    now.into(),
                ]);
                rows.push(format!(
                    "({})",
                    (start..start + 6)
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            conn.execute(self.db.stmt(
                &format!(
                    "INSERT INTO monoize_channel_models
                     (id, channel_id, model_name, redirect, multiplier, created_at)
                     VALUES {}",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

struct ExistingChannel {
    api_key: String,
}

struct PreparedChannel<'a> {
    id: String,
    api_key: String,
    input: &'a CreateMonoizeChannelInput,
    models: HashMap<String, MonoizeModelEntry>,
    chat_dialect: Option<ChatDialect>,
}

fn prepare_channels<'a>(
    channels: &'a [CreateMonoizeChannelInput],
    existing_channels: &HashMap<String, ExistingChannel>,
) -> Result<Vec<PreparedChannel<'a>>, String> {
    let mut prepared = Vec::with_capacity(channels.len());
    for input in channels {
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| format!("mono_ch_{}", uuid::Uuid::new_v4().simple()));

        let api_key = match input.api_key.as_deref() {
            Some(k) if !k.trim().is_empty() => k.to_string(),
            _ => existing_channels
                .get(&id)
                .map(|c| c.api_key.clone())
                .ok_or_else(|| {
                    format!(
                        "channel api_key must not be empty for new channel '{}'",
                        input.name
                    )
                })?,
        };
        prepared.push(PreparedChannel {
            id,
            api_key,
            input,
            models: canonicalize_models(&input.models),
            chat_dialect: channel_chat_dialect(input)?,
        });
    }

    Ok(prepared)
}

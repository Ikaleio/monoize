use super::catalog::RoutingCatalog;
use super::decode::*;
use super::types::*;
use crate::db::DbPool;
use crate::db_cache::{REQUEST_PATH_CACHE_TTL, SnapshotCache};
use crate::users::canonicalize_group_ids;
use chrono::Utc;
use sea_orm::{ConnectionTrait, Value as SeaValue};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone)]
pub struct MonoizeRoutingStore {
    pub(super) db: DbPool,
    pub(super) catalog: SnapshotCache<RoutingCatalog>,
}

impl MonoizeRoutingStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        let store = Self::with_empty_catalog(db);
        store.migrate_transform_rule_ids().await?;
        Ok(store)
    }

    /// Replica-side constructor per PRP11: skips canonicalization writes that the
    /// primary already performed on the shared database.
    pub async fn new_read_only(db: DbPool) -> Result<Self, String> {
        Ok(Self::with_empty_catalog(db))
    }

    fn with_empty_catalog(db: DbPool) -> Self {
        Self {
            db,
            catalog: SnapshotCache::new(REQUEST_PATH_CACHE_TTL),
        }
    }

    /// DPT-RC4: callers that write routing rows outside this store (the
    /// group-delete cascade) invalidate through this method.
    pub fn invalidate_routing_catalog(&self) {
        self.catalog.invalidate();
    }

    async fn routing_catalog(&self) -> Result<Arc<RoutingCatalog>, String> {
        let store = self.clone();
        self.catalog
            .get_or_load(
                move || async move { Ok(RoutingCatalog::build(store.list_providers().await?)) },
            )
            .await
    }

    pub async fn provider_count(&self) -> Result<i64, String> {
        let row = self
            .db
            .read()
            .query_one(
                self.db
                    .stmt("SELECT COUNT(*) as cnt FROM monoize_providers", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no rows".to_string())?;
        row.try_get("", "cnt").map_err(|e| e.to_string())
    }

    async fn load_channels_bulk(
        &self,
        provider_id: Option<&str>,
    ) -> Result<HashMap<String, Vec<MonoizeChannel>>, String> {
        let provider_filter = if provider_id.is_some() {
            " WHERE provider_id = $1"
        } else {
            ""
        };
        let values = provider_id.map(|id| vec![id.into()]).unwrap_or_default();
        let channel_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, provider_id, name, base_url, api_key, weight, enabled,
                            provider_type, passive_failure_count_threshold_override,
                            passive_cooldown_seconds_override, passive_window_seconds_override,
                            passive_rate_limit_cooldown_seconds_override,
                            active_probe_enabled_override, active_probe_interval_seconds_override,
                            active_probe_success_threshold_override, active_probe_model_override,
                            affinity_enabled_override, affinity_idle_ttl_seconds_override,
                            affinity_failback_mode_override, affinity_failback_delay_seconds_override,
                            proxy_url, extra_headers, session_affinity_auto, websocket_supported,
                            chat_dialect
                     FROM monoize_channels{provider_filter}
                     ORDER BY created_at ASC"
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;

        let (model_sql, model_values) = if let Some(provider_id) = provider_id {
            (
                "SELECT cm.channel_id, cm.model_name, cm.redirect, cm.multiplier
                 FROM monoize_channel_models cm
                 JOIN monoize_channels c ON c.id = cm.channel_id
                 WHERE c.provider_id = $1
                 ORDER BY cm.channel_id ASC, cm.model_name ASC",
                vec![provider_id.into()],
            )
        } else {
            (
                "SELECT channel_id, model_name, redirect, multiplier
                 FROM monoize_channel_models
                 ORDER BY channel_id ASC, model_name ASC",
                vec![],
            )
        };
        let model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(model_sql, model_values))
            .await
            .map_err(|e| e.to_string())?;
        let mut models_by_channel: HashMap<String, HashMap<String, MonoizeModelEntry>> =
            HashMap::new();
        for row in model_rows {
            let channel_id: String = row.try_get("", "channel_id").map_err(|e| e.to_string())?;
            let model_name: String = row.try_get("", "model_name").map_err(|e| e.to_string())?;
            let multiplier = row
                .try_get::<String>("", "multiplier")
                .map_err(|e| e.to_string())?
                .parse()
                .map_err(|e: String| format!("channel {channel_id} invalid multiplier: {e}"))?;
            models_by_channel.entry(channel_id).or_default().insert(
                model_name,
                MonoizeModelEntry {
                    redirect: row.try_get("", "redirect").map_err(|e| e.to_string())?,
                    multiplier,
                },
            );
        }

        let mut channels_by_provider: HashMap<String, Vec<MonoizeChannel>> = HashMap::new();
        for row in channel_rows {
            let provider_id: String = row.try_get("", "provider_id").map_err(|e| e.to_string())?;
            let channel_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            channels_by_provider
                .entry(provider_id)
                .or_default()
                .push(decode_channel_row(
                    &row,
                    models_by_channel.remove(&channel_id).unwrap_or_default(),
                )?);
        }
        Ok(channels_by_provider)
    }

    pub async fn list_providers(&self) -> Result<Vec<MonoizeProvider>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist,
                          strip_cross_protocol_nested_extra, group_ids,
                          allow_free_when_unpriced_override,
                          allow_free_when_missing_usage_override,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   ORDER BY priority ASC, created_at ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut channels_by_provider = self.load_channels_bulk(None).await?;
        rows.iter()
            .map(|row| {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                decode_provider_row(row, channels_by_provider.remove(&id).unwrap_or_default())
            })
            .collect()
    }

    /// DPT-RC3: the candidates served by an enabled provider/channel.
    pub async fn available_model_names(
        &self,
        candidates: &[String],
    ) -> Result<HashSet<String>, String> {
        if candidates.is_empty() {
            return Ok(HashSet::new());
        }
        let catalog = self.routing_catalog().await?;
        Ok(candidates
            .iter()
            .filter(|candidate| catalog.contains_model(candidate))
            .cloned()
            .collect())
    }

    pub async fn list_available_model_names(&self) -> Result<Vec<String>, String> {
        Ok(self.routing_catalog().await?.model_names.clone())
    }

    /// DPT-RC3: providers serving `model`, each reduced to the qualifying
    /// channels with `models` holding only the `model` entry.
    pub async fn list_providers_for_model(
        &self,
        model: &str,
    ) -> Result<Arc<[MonoizeProvider]>, String> {
        Ok(self.routing_catalog().await?.providers_for_model(model))
    }

    pub async fn list_active_probe_candidates(&self) -> Result<Vec<MonoizeProvider>, String> {
        let provider_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT id, name, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist,
                          strip_cross_protocol_nested_extra, group_ids,
                          allow_free_when_unpriced_override,
                          allow_free_when_missing_usage_override,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   WHERE circuit_breaker_enabled = 1
                     AND enabled = 1
                     AND EXISTS (
                         SELECT 1
                         FROM monoize_channels c
                         WHERE c.provider_id = monoize_providers.id
                           AND c.enabled = 1
                           AND c.weight > 0
                     )
                   ORDER BY priority ASC, created_at ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if provider_rows.is_empty() {
            return Ok(Vec::new());
        }
        let channel_model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                r#"SELECT c.id, c.provider_id, c.name, c.base_url, c.api_key, c.weight, c.enabled,
                          c.provider_type, c.passive_failure_count_threshold_override,
                          c.passive_cooldown_seconds_override, c.passive_window_seconds_override,
                          c.passive_rate_limit_cooldown_seconds_override,
                          c.active_probe_enabled_override, c.active_probe_interval_seconds_override,
                          c.active_probe_success_threshold_override, c.active_probe_model_override,
                          c.affinity_enabled_override, c.affinity_idle_ttl_seconds_override,
                          c.affinity_failback_mode_override, c.affinity_failback_delay_seconds_override,
                          c.proxy_url, c.extra_headers, c.session_affinity_auto, c.websocket_supported,
                          c.chat_dialect, cm.model_name, cm.redirect, cm.multiplier
                   FROM monoize_channels c
                   JOIN monoize_providers p ON p.id = c.provider_id
                   JOIN monoize_channel_models cm ON cm.channel_id = c.id
                   WHERE p.circuit_breaker_enabled = 1
                     AND p.enabled = 1
                     AND c.enabled = 1
                     AND c.weight > 0
                   ORDER BY c.created_at ASC, cm.model_name ASC"#,
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut channels_by_provider: HashMap<String, Vec<MonoizeChannel>> = HashMap::new();
        let mut channel_positions: HashMap<(String, String), usize> = HashMap::new();
        for row in channel_model_rows {
            let provider_id: String = row.try_get("", "provider_id").map_err(|e| e.to_string())?;
            let channel_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let model_name: String = row.try_get("", "model_name").map_err(|e| e.to_string())?;
            let channels = channels_by_provider.entry(provider_id.clone()).or_default();
            if let Some(index) = channel_positions.get(&(provider_id.clone(), channel_id.clone())) {
                let model_entry = decode_channel_model_row(&row, &model_name)?
                    .models
                    .remove(&model_name)
                    .expect("decoded channel model row must contain its model");
                channels[*index].models.insert(model_name, model_entry);
            } else {
                let index = channels.len();
                channels.push(decode_channel_model_row(&row, &model_name)?);
                channel_positions.insert((provider_id, channel_id), index);
            }
        }
        provider_rows
            .iter()
            .map(|row| {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                decode_provider_row(row, channels_by_provider.remove(&id).unwrap_or_default())
            })
            .collect()
    }

    /// GR-I2/GR-C3 for the provider group set: canonicalize, replace an empty
    /// selection with the default group id, and require every id to reference
    /// an existing registry row.
    pub(super) async fn resolve_provider_group_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<String>, String> {
        let group_ids = canonicalize_group_ids(group_ids);
        if group_ids.len() > 32 {
            return Err("at most 32 groups can be selected".to_string());
        }
        if group_ids.is_empty() {
            let row = self
                .db
                .read()
                .query_one(self.db.stmt(
                    "SELECT id FROM monoize_groups WHERE is_default = 1 LIMIT 1",
                    vec![],
                ))
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "default group row missing (GR-D2 violated)".to_string())?;
            let default_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            return Ok(vec![default_id]);
        }
        for id in &group_ids {
            let row = self
                .db
                .read()
                .query_one(self.db.stmt(
                    "SELECT 1 AS one FROM monoize_groups WHERE id = $1",
                    vec![id.clone().into()],
                ))
                .await
                .map_err(|e| e.to_string())?;
            if row.is_none() {
                return Err(format!("unknown group id: {id}"));
            }
        }
        Ok(group_ids)
    }

    /// WS2e: write Channel WebSocket memory only when the stored value is NULL.
    /// Returns true when this statement changed a row.
    pub async fn remember_channel_websocket_supported(
        &self,
        channel_id: &str,
        supported: bool,
    ) -> Result<bool, String> {
        let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
        let result = tx
            .execute(self.db.stmt(
                "UPDATE monoize_channels
                 SET websocket_supported = $1, updated_at = $2
                 WHERE id = $3 AND websocket_supported IS NULL",
                vec![
                    opt_bool_to_value(Some(supported)),
                    Utc::now().to_rfc3339().into(),
                    channel_id.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let changed = result.rows_affected() > 0;
        if changed {
            self.catalog.invalidate();
        }
        Ok(changed)
    }

    pub async fn get_provider(&self, id: &str) -> Result<Option<MonoizeProvider>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                r#"SELECT id, name, max_retries, channel_max_retries,
                          channel_retry_interval_ms, circuit_breaker_enabled,
                          per_model_circuit_break, transforms, api_type_overrides,
                          active_probe_enabled_override, active_probe_interval_seconds_override,
                          active_probe_success_threshold_override, active_probe_model_override,
                          request_timeout_ms_override, extra_fields_whitelist,
                          strip_cross_protocol_nested_extra, group_ids,
                          allow_free_when_unpriced_override,
                          allow_free_when_missing_usage_override,
                          enabled, priority, created_at, updated_at
                   FROM monoize_providers
                   WHERE id = $1"#,
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let Some(row) = row else {
            return Ok(None);
        };
        let mut channels_by_provider = self.load_channels_bulk(Some(id)).await?;
        Ok(Some(decode_provider_row(
            &row,
            channels_by_provider.remove(id).unwrap_or_default(),
        )?))
    }
}

pub(super) fn opt_bool_to_value(v: Option<bool>) -> SeaValue {
    match v {
        Some(b) => SeaValue::Int(Some(if b { 1 } else { 0 })),
        None => SeaValue::Int(None),
    }
}

pub(super) fn opt_u64_to_value(v: Option<u64>) -> SeaValue {
    match v {
        Some(n) => SeaValue::Int(Some(n as i32)),
        None => SeaValue::Int(None),
    }
}

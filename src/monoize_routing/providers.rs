use super::decode::serialize_provider_group_ids_json;
use super::runtime::provider_reorder_max_ids;
use super::store::MonoizeRoutingStore;
use super::types::*;
use super::validation::*;
use super::values::*;
use crate::transforms::canonicalize_transform_rules;
use chrono::Utc;
use sea_orm::{ConnectionTrait, Value as SeaValue};
use std::collections::HashSet;

impl MonoizeRoutingStore {
    pub async fn create_provider(
        &self,
        input: CreateMonoizeProviderInput,
    ) -> Result<MonoizeProvider, String> {
        validate_create_provider_input(&input)?;

        let id = generate_short_id();
        let now = Utc::now();
        // Resolve before begin_write: the registry lookup uses the read pool,
        // which on single-connection SQLite would deadlock behind our own
        // write transaction.
        let group_ids_json = serialize_provider_group_ids_json(
            &self.resolve_provider_group_ids(&input.group_ids).await?,
        )?;
        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;

        let priority = self.next_provider_priority(&*txn, input.priority).await?;

        let mut transforms = input.transforms.clone();
        canonicalize_transform_rules(&mut transforms);
        let transforms_json = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
        let api_type_overrides_json =
            serde_json::to_string(&stored_api_type_overrides(&input.api_type_overrides)?)
                .map_err(|e| e.to_string())?;
        let extra_fields_whitelist_json: Option<String> = input
            .extra_fields_whitelist
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
        let strip_cross_proto = input.strip_cross_protocol_nested_extra;

        txn.execute(self.db.stmt(
                r#"INSERT INTO monoize_providers (
                        id, name, max_retries, channel_max_retries,
                        channel_retry_interval_ms, circuit_breaker_enabled,
                        per_model_circuit_break, transforms, api_type_overrides,
                        active_probe_enabled_override, active_probe_interval_seconds_override,
                        active_probe_success_threshold_override, active_probe_model_override,
                        request_timeout_ms_override, extra_fields_whitelist,
                        strip_cross_protocol_nested_extra, group_ids,
                        allow_free_when_unpriced_override,
                        allow_free_when_missing_usage_override,
                        enabled, priority, created_at, updated_at
                   ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)"#,
                vec![
                        id.clone().into(),
                        input.name.clone().into(),
                        SeaValue::Int(Some(input.max_retries)),
                        SeaValue::Int(Some(input.channel_max_retries)),
                        SeaValue::Int(Some(input.channel_retry_interval_ms)),
                        SeaValue::Int(Some(if input.circuit_breaker_enabled { 1 } else { 0 })),
                        SeaValue::Int(Some(if input.per_model_circuit_break { 1 } else { 0 })),
                        transforms_json.into(),
                        api_type_overrides_json.into(),
                        opt_bool_to_value(input.active_probe_enabled_override),
                        opt_u64_to_value(input.active_probe_interval_seconds_override),
                        opt_u64_to_value(
                            input
                                .active_probe_success_threshold_override
                                .map(|v| v as u64),
                        ),
                        input.active_probe_model_override.clone().into(),
                        opt_u64_to_value(input.request_timeout_ms_override),
                        extra_fields_whitelist_json.into(),
                        opt_bool_to_value(strip_cross_proto),
                        group_ids_json.into(),
                        opt_bool_to_value(input.allow_free_when_unpriced_override),
                        opt_bool_to_value(input.allow_free_when_missing_usage_override),
                        SeaValue::Int(Some(if input.enabled { 1 } else { 0 })),
                        SeaValue::Int(Some(priority)),
                        now.to_rfc3339().into(),
                        now.to_rfc3339().into(),
                    ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        self.replace_channels_on(&*txn, &id, &input.channels)
            .await?;
        txn.commit().await.map_err(|e| e.to_string())?;
        self.catalog.invalidate();

        self.get_provider(&id)
            .await?
            .ok_or_else(|| "provider not found after create".to_string())
    }

    pub async fn update_provider(
        &self,
        id: &str,
        input: UpdateMonoizeProviderInput,
    ) -> Result<MonoizeProvider, String> {
        validate_update_provider_input(&input)?;

        let mut update = ProviderUpdate::from_input(&input)?;
        self.add_provider_overrides(&input, &mut update).await?;
        update.push("updated_at", Utc::now().to_rfc3339().into());
        let ProviderUpdate {
            set_clauses,
            mut values,
        } = update;

        let id_index = values.len() + 1;
        values.push(id.into());
        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;
        let result = txn
            .execute(self.db.stmt(
                &format!(
                    "UPDATE monoize_providers SET {} WHERE id = ${id_index}",
                    set_clauses.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() == 0 {
            return Err("provider not found".to_string());
        }

        if let Some(channels) = &input.channels {
            self.replace_channels_on(&*txn, id, channels).await?;
        }

        txn.commit().await.map_err(|e| e.to_string())?;
        self.catalog.invalidate();

        self.get_provider(id)
            .await?
            .ok_or_else(|| "provider not found after update".to_string())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<(), String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM monoize_providers WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if result.rows_affected() == 0 {
            return Err("provider not found".to_string());
        }

        self.catalog.invalidate();
        Ok(())
    }

    pub async fn reorder_providers(&self, input: ReorderProvidersInput) -> Result<(), String> {
        if input.provider_ids.len() > provider_reorder_max_ids() {
            return Err(format!(
                "provider reorder accepts at most {} ids",
                provider_reorder_max_ids()
            ));
        }
        let mut uniq = HashSet::new();
        for id in &input.provider_ids {
            if !uniq.insert(id.clone()) {
                return Err("provider_ids contains duplicates".to_string());
            }
        }

        let txn = self.db.begin_write().await.map_err(|e| e.to_string())?;
        if self.db.is_postgres() {
            txn.execute_unprepared("LOCK TABLE monoize_providers IN SHARE ROW EXCLUSIVE MODE")
                .await
                .map_err(|e| e.to_string())?;
        }
        let rows = txn
            .query_all(
                self.db
                    .stmt("SELECT id FROM monoize_providers ORDER BY id", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?;
        if rows.len() != input.provider_ids.len() {
            return Err("provider_ids must contain all providers exactly once".to_string());
        }

        let existing_ids: HashSet<String> = rows
            .into_iter()
            .map(|row| row.try_get("", "id").map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let input_ids: HashSet<String> = input.provider_ids.iter().cloned().collect();
        if existing_ids != input_ids {
            return Err("provider_ids must contain all providers exactly once".to_string());
        }
        if input.provider_ids.is_empty() {
            return txn.commit().await.map_err(|e| e.to_string());
        }

        let mut values = Vec::with_capacity(input.provider_ids.len() * 2 + 1);
        let mut cases = Vec::with_capacity(input.provider_ids.len());
        for (priority, id) in input.provider_ids.iter().enumerate() {
            let id_index = values.len() + 1;
            values.push(id.clone().into());
            let priority_index = values.len() + 1;
            values.push(SeaValue::Int(Some(priority as i32)));
            cases.push(format!("WHEN ${id_index} THEN ${priority_index}"));
        }
        let updated_at_index = values.len() + 1;
        values.push(Utc::now().to_rfc3339().into());
        txn.execute(self.db.stmt(
            &format!(
                "UPDATE monoize_providers
                 SET priority = CASE id {} END, updated_at = ${updated_at_index}",
                cases.join(" ")
            ),
            values,
        ))
        .await
        .map_err(|e| e.to_string())?;
        txn.commit().await.map_err(|e| e.to_string())?;
        self.catalog.invalidate();
        Ok(())
    }

    async fn next_provider_priority(
        &self,
        txn: &impl ConnectionTrait,
        requested: Option<i32>,
    ) -> Result<i32, String> {
        match requested {
            Some(v) => Ok(v),
            None => {
                if self.db.is_postgres() {
                    txn.execute_unprepared(
                        "LOCK TABLE monoize_providers IN SHARE ROW EXCLUSIVE MODE",
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                let row = txn
                    .query_one(self.db.stmt(
                        "SELECT CAST(MAX(priority) AS BIGINT) AS max_p FROM monoize_providers",
                        vec![],
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                let max_priority = row
                    .map(|row| {
                        row.try_get::<Option<i64>>("", "max_p")
                            .map_err(|e| e.to_string())
                    })
                    .transpose()?
                    .flatten();
                let next_priority = max_priority
                    .unwrap_or(-1)
                    .checked_add(1)
                    .ok_or_else(|| "provider priority overflow".to_string())?;
                i32::try_from(next_priority)
                    .map_err(|_| "provider priority exceeds signed 32-bit range".to_string())
            }
        }
    }

    async fn add_provider_overrides(
        &self,
        input: &UpdateMonoizeProviderInput,
        update: &mut ProviderUpdate,
    ) -> Result<(), String> {
        if let Some(value) = input.active_probe_enabled_override {
            update.push("active_probe_enabled_override", opt_bool_to_value(value));
        }
        if let Some(value) = input.active_probe_interval_seconds_override {
            update.push(
                "active_probe_interval_seconds_override",
                opt_u64_to_value(value),
            );
        }
        if let Some(value) = input.active_probe_success_threshold_override {
            update.push(
                "active_probe_success_threshold_override",
                opt_u64_to_value(value.map(u64::from)),
            );
        }
        if let Some(value) = &input.active_probe_model_override {
            update.push("active_probe_model_override", value.clone().into());
        }
        if let Some(value) = input.request_timeout_ms_override {
            update.push("request_timeout_ms_override", opt_u64_to_value(value));
        }
        if let Some(value) = &input.extra_fields_whitelist {
            let encoded = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| e.to_string())?;
            update.push("extra_fields_whitelist", encoded.into());
        }
        if let Some(value) = input.strip_cross_protocol_nested_extra {
            update.push(
                "strip_cross_protocol_nested_extra",
                opt_bool_to_value(value),
            );
        }
        if let Some(value) = &input.group_ids {
            let resolved = self.resolve_provider_group_ids(value).await?;
            update.push(
                "group_ids",
                serialize_provider_group_ids_json(&resolved)?.into(),
            );
        }
        if let Some(value) = input.allow_free_when_unpriced_override {
            update.push(
                "allow_free_when_unpriced_override",
                opt_bool_to_value(value),
            );
        }
        if let Some(value) = input.allow_free_when_missing_usage_override {
            update.push(
                "allow_free_when_missing_usage_override",
                opt_bool_to_value(value),
            );
        }
        if let Some(value) = input.enabled {
            update.push("enabled", SeaValue::Int(Some(if value { 1 } else { 0 })));
        }
        if let Some(value) = input.priority {
            update.push("priority", SeaValue::Int(Some(value)));
        }
        Ok(())
    }
}

#[derive(Default)]
struct ProviderUpdate {
    set_clauses: Vec<String>,
    values: Vec<SeaValue>,
}

impl ProviderUpdate {
    fn push(&mut self, column: &str, value: SeaValue) {
        let index = self.values.len() + 1;
        self.set_clauses.push(format!("{column} = ${index}"));
        self.values.push(value);
    }

    fn from_input(input: &UpdateMonoizeProviderInput) -> Result<Self, String> {
        let mut update = Self::default();
        if let Some(value) = &input.name {
            update.push("name", value.clone().into());
        }
        if let Some(value) = input.max_retries {
            update.push("max_retries", SeaValue::Int(Some(value)));
        }
        if let Some(value) = input.channel_max_retries {
            update.push("channel_max_retries", SeaValue::Int(Some(value)));
        }
        if let Some(value) = input.channel_retry_interval_ms {
            update.push("channel_retry_interval_ms", SeaValue::Int(Some(value)));
        }
        if let Some(value) = input.circuit_breaker_enabled {
            update.push(
                "circuit_breaker_enabled",
                SeaValue::Int(Some(if value { 1 } else { 0 })),
            );
        }
        if let Some(value) = input.per_model_circuit_break {
            update.push(
                "per_model_circuit_break",
                SeaValue::Int(Some(if value { 1 } else { 0 })),
            );
        }
        if let Some(mut transforms) = input.transforms.clone() {
            canonicalize_transform_rules(&mut transforms);
            update.push(
                "transforms",
                serde_json::to_string(&transforms)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
        }
        if let Some(value) = &input.api_type_overrides {
            update.push(
                "api_type_overrides",
                serde_json::to_string(&stored_api_type_overrides(value)?)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
        }
        Ok(update)
    }
}

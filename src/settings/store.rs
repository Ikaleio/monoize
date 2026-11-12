use super::*;

impl SettingsStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        let store = Self { db };
        store.ensure_defaults().await?;
        store.migrate_transform_rule_ids().await?;
        Ok(store)
    }

    /// Replica-side constructor per PRP11: performs no writes because defaults and
    /// canonicalization markers are guaranteed to exist once the primary has started.
    pub async fn new_read_only(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    async fn ensure_defaults(&self) -> Result<(), String> {
        let defaults = SystemSettings::default();
        self.ensure_core_defaults(&defaults).await?;
        self.ensure_capture_and_billing_defaults(&defaults).await
    }

    async fn set_if_not_exists(&self, key: &str, value: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        // INSERT ... ON CONFLICT DO NOTHING — works cross-DB via sea-query
        let insert = system_settings::Entity::insert(system_settings::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(system_settings::Column::Key)
                .do_nothing()
                .to_owned(),
        )
        .do_nothing();

        let _write_guard = self.db.write().await;
        insert
            .exec(&*_write_guard)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let row = system_settings::Entity::find_by_id(key.to_string())
            .one(self.db.read())
            .await
            .map_err(|e| e.to_string())?;

        Ok(row.map(|r| r.value))
    }

    pub async fn get_session_ttl_days(&self) -> Result<i64, String> {
        Ok(self
            .get("session_ttl_days")
            .await?
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(7))
    }

    pub async fn get_api_key_max_per_user(&self) -> Result<i64, String> {
        Ok(self
            .get("api_key_max_per_user")
            .await?
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1000))
    }

    pub async fn get_public_settings(&self) -> Result<PublicSettings, String> {
        const PUBLIC_KEYS: [&str; 5] = [
            "registration_enabled",
            "captcha_enabled",
            "site_name",
            "site_description",
            "api_base_url",
        ];
        let rows = system_settings::Entity::find()
            .filter(system_settings::Column::Key.is_in(PUBLIC_KEYS))
            .all(self.db.read())
            .await
            .map_err(|error| error.to_string())?;
        let defaults = SystemSettings::default();
        let mut public = PublicSettings {
            registration_enabled: defaults.registration_enabled,
            captcha_enabled: defaults.captcha_enabled,
            site_name: defaults.site_name,
            site_description: defaults.site_description,
            api_base_url: defaults.api_base_url,
            cap_api_endpoint: None,
        };
        for row in rows {
            match row.key.as_str() {
                "registration_enabled" => {
                    public.registration_enabled = decode_registration_enabled(&row.value)?;
                }
                "captcha_enabled" => {
                    public.captcha_enabled = decode_captcha_enabled(&row.value)?;
                }
                "site_name" => public.site_name = row.value,
                "site_description" => public.site_description = row.value,
                "api_base_url" => public.api_base_url = row.value,
                _ => {}
            }
        }
        Ok(public)
    }

    pub async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();

        let model = system_settings::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            updated_at: Set(now),
        };

        let insert = system_settings::Entity::insert(model).on_conflict(
            OnConflict::column(system_settings::Column::Key)
                .update_columns([
                    system_settings::Column::Value,
                    system_settings::Column::UpdatedAt,
                ])
                .to_owned(),
        );

        let _write_guard = self.db.write().await;
        insert
            .exec(&*_write_guard)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn get_all(&self) -> Result<SystemSettings, String> {
        let rows = system_settings::Entity::find()
            .all(self.db.read())
            .await
            .map_err(|e| e.to_string())?;

        let mut settings = SystemSettings::default();
        let mut latest_update = settings.updated_at;

        for row in rows {
            if let Ok(updated_at) = DateTime::parse_from_rfc3339(&row.updated_at) {
                let updated_at = updated_at.with_timezone(&Utc);
                if updated_at > latest_update {
                    latest_update = updated_at;
                }
            }

            if row.key.starts_with("monoize_") {
                apply_runtime_setting(&mut settings, row)?;
            } else {
                apply_general_setting(&mut settings, row)?;
            }
        }

        settings.updated_at = latest_update;
        Ok(settings)
    }

    pub async fn update_all(&self, settings: &SystemSettings) -> Result<SystemSettings, String> {
        let mut settings = settings.clone();
        canonicalize_transform_rules(&mut settings.global_transforms);
        canonicalize_codex_model_ids(&mut settings.codex_model_ids);
        canonicalize_ordered_string_ids(&mut settings.dashboard_performance_group_ids);
        canonicalize_ordered_string_ids(&mut settings.dashboard_performance_model_ids);
        settings.monoize_request_capture_max_total_bytes =
            clamp_request_capture_max_total_bytes(settings.monoize_request_capture_max_total_bytes);
        settings.monoize_affinity_idle_ttl_seconds =
            settings.monoize_affinity_idle_ttl_seconds.max(1);
        let values = settings_values(&settings)?;

        let committed_at = Utc::now();
        let updated_at = committed_at.to_rfc3339();
        let models = values
            .into_iter()
            .map(|(key, value)| system_settings::ActiveModel {
                key: Set(key.to_string()),
                value: Set(value),
                updated_at: Set(updated_at.clone()),
            })
            .collect::<Vec<_>>();
        let transaction = self.db.begin_write().await.map_err(|e| e.to_string())?;
        system_settings::Entity::insert_many(models)
            .on_conflict(
                OnConflict::column(system_settings::Column::Key)
                    .update_columns([
                        system_settings::Column::Value,
                        system_settings::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&*transaction)
            .await
            .map_err(|e| e.to_string())?;
        bump_config_epoch_in_tx(&self.db, &transaction).await?;
        transaction.commit().await.map_err(|e| e.to_string())?;
        settings.updated_at = committed_at;
        Ok(settings)
    }

    async fn migrate_transform_rule_ids(&self) -> Result<(), String> {
        let Some(raw) = self.get("global_transforms").await? else {
            return Ok(());
        };
        let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&raw) else {
            tracing::warn!("skip invalid global_transforms during transform id migration");
            return Ok(());
        };
        if !canonicalize_transform_rules(&mut transforms) {
            return Ok(());
        }
        self.set(
            "global_transforms",
            &serde_json::to_string(&transforms).map_err(|e| e.to_string())?,
        )
        .await
    }

    pub async fn is_registration_enabled(&self) -> Result<bool, String> {
        match self.get("registration_enabled").await? {
            Some(raw) => decode_registration_enabled(&raw),
            None => Ok(true),
        }
    }

    pub async fn is_captcha_enabled(&self) -> Result<bool, String> {
        match self.get("captcha_enabled").await? {
            Some(raw) => decode_captcha_enabled(&raw),
            None => Ok(true),
        }
    }

    pub async fn get_reasoning_suffix_map(&self) -> Result<HashMap<String, String>, String> {
        match self.get("reasoning_suffix_map").await? {
            Some(json_str) => serde_json::from_str(&json_str)
                .map_err(|e| format!("invalid reasoning_suffix_map JSON: {e}")),
            None => Ok(default_reasoning_suffix_map()),
        }
    }
}

impl SettingsStore {
    async fn ensure_core_defaults(&self, defaults: &SystemSettings) -> Result<(), String> {
        self.set_if_not_exists(
            "registration_enabled",
            &serde_json::to_string(&defaults.registration_enabled).unwrap(),
        )
        .await?;
        self.set_if_not_exists("captcha_enabled", &defaults.captcha_enabled.to_string())
            .await?;
        self.set_if_not_exists("session_ttl_days", &defaults.session_ttl_days.to_string())
            .await?;
        self.set_if_not_exists(
            "api_key_max_per_user",
            &defaults.api_key_max_per_user.to_string(),
        )
        .await?;
        self.set_if_not_exists("site_name", &defaults.site_name)
            .await?;
        self.set_if_not_exists("site_description", &defaults.site_description)
            .await?;
        self.set_if_not_exists("api_base_url", &defaults.api_base_url)
            .await?;
        self.set_if_not_exists(
            "global_transforms",
            &serde_json::to_string(&defaults.global_transforms)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "global_model_redirects",
            &serde_json::to_string(&defaults.global_model_redirects)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "reasoning_suffix_map",
            &serde_json::to_string(&defaults.reasoning_suffix_map).unwrap(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_enabled",
            &defaults.monoize_active_probe_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_interval_seconds",
            &defaults.monoize_active_probe_interval_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_success_threshold",
            &defaults.monoize_active_probe_success_threshold.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_active_probe_model",
            &defaults
                .monoize_active_probe_model
                .clone()
                .unwrap_or_default(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_failure_threshold",
            &defaults.monoize_passive_failure_threshold.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_cooldown_seconds",
            &defaults.monoize_passive_cooldown_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_window_seconds",
            &defaults.monoize_passive_window_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_passive_rate_limit_cooldown_seconds",
            &defaults
                .monoize_passive_rate_limit_cooldown_seconds
                .to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_request_timeout_ms",
            &defaults.monoize_request_timeout_ms.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_stream_idle_timeout_ms",
            &defaults.monoize_stream_idle_timeout_ms.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_enable_estimated_billing",
            &defaults.monoize_enable_estimated_billing.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_extra_fields_whitelist",
            &serde_json::to_string(&defaults.monoize_extra_fields_whitelist)
                .unwrap_or_else(|_| "{}".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_strip_cross_protocol_nested_extra",
            &defaults
                .monoize_strip_cross_protocol_nested_extra
                .to_string(),
        )
        .await?;
        Ok(())
    }
    async fn ensure_capture_and_billing_defaults(
        &self,
        defaults: &SystemSettings,
    ) -> Result<(), String> {
        self.set_if_not_exists(
            "monoize_request_capture_enabled",
            &defaults.monoize_request_capture_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_request_capture_max_total_bytes",
            &defaults.monoize_request_capture_max_total_bytes.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_mask_sensitive_info",
            &defaults.monoize_mask_sensitive_info.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_enabled",
            &defaults.monoize_affinity_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_idle_ttl_seconds",
            &defaults.monoize_affinity_idle_ttl_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_failback_mode",
            defaults.monoize_affinity_failback_mode.as_str(),
        )
        .await?;
        self.set_if_not_exists(
            "monoize_affinity_failback_delay_seconds",
            &defaults.monoize_affinity_failback_delay_seconds.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "allow_free_when_unpriced",
            &defaults.allow_free_when_unpriced.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "allow_free_when_missing_usage",
            &defaults.allow_free_when_missing_usage.to_string(),
        )
        .await?;
        self.set_if_not_exists(
            "tool_prices",
            &serde_json::to_string(&defaults.tool_prices).unwrap_or_else(|_| "{}".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "price_sync_new_api_base_url",
            &defaults.price_sync_new_api_base_url,
        )
        .await?;
        self.set_if_not_exists(
            "price_sync_new_api_token",
            &defaults.price_sync_new_api_token,
        )
        .await?;
        self.set_if_not_exists(
            "price_sync_auto_enabled",
            &defaults.price_sync_auto_enabled.to_string(),
        )
        .await?;
        self.set_if_not_exists("recharge_public_origin", &defaults.recharge_public_origin)
            .await?;
        self.set_if_not_exists(
            "dashboard_performance_group_ids",
            &serde_json::to_string(&defaults.dashboard_performance_group_ids)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        self.set_if_not_exists(
            "dashboard_performance_model_ids",
            &serde_json::to_string(&defaults.dashboard_performance_model_ids)
                .unwrap_or_else(|_| "[]".to_string()),
        )
        .await?;
        Ok(())
    }
}

mod values;
use values::*;

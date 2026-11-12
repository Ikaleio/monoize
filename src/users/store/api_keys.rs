use super::*;

impl UserStore {
    pub async fn create_api_key(
        &self,
        user_id: &str,
        name: &str,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(ApiKey, String), String> {
        self.create_api_key_extended(
            user_id,
            CreateApiKeyInput {
                name: name.to_string(),
                expires_in_days: expires_at.map(|e| (e - Utc::now()).num_days()),
                sub_account_enabled: false,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                use_user_group: true,
                group_ids: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
                reasoning_envelope_enabled: true,
                request_capture_mode: RequestCaptureMode::Off,
                request_capture_retention: RequestCaptureRetention::default(),
            },
            false,
        )
        .await
    }

    pub async fn create_api_key_extended(
        &self,
        user_id: &str,
        mut input: CreateApiKeyInput,
        is_admin: bool,
    ) -> Result<(ApiKey, String), String> {
        validate_positive_max_multiplier(input.max_multiplier)?;
        canonicalize_transform_rules(&mut input.transforms);
        validate_api_key_transforms(&input.transforms, is_admin, &self.custom_transforms.get())?;
        let compiled_model_redirects = compile_model_redirects(&input.model_redirects)?;
        input.ip_whitelist = canonicalize_ip_whitelist(&input.ip_whitelist)?;
        if input.sub_account_balance_nano_usd.is_some() && !is_admin {
            return Err("only admins may set an initial sub-account balance".to_string());
        }
        let initial_sub_account_balance = match input.sub_account_balance_nano_usd.as_deref() {
            Some(raw) => {
                let parsed = parse_nano_usd(raw)?;
                if raw != parsed.to_string() || parsed < 0 {
                    return Err(
                        "initial sub-account balance must be a canonical non-negative integer"
                            .to_string(),
                    );
                }
                parsed
            }
            None => 0,
        };
        if initial_sub_account_balance != 0 && !input.sub_account_enabled {
            return Err(
                "a non-zero sub-account balance requires sub-account billing to be enabled"
                    .to_string(),
            );
        }
        let owner_group_id = self
            .get_user_by_id(user_id)
            .await?
            .map(|user| user.group_id)
            .unwrap_or_default();
        // TM-GRP-4: an inheriting key stores no explicit selection.
        let group_ids = if input.use_user_group {
            Vec::new()
        } else {
            let group_ids = canonicalize_group_ids(&input.group_ids);
            if group_ids.is_empty() {
                return Err(
                    "group_ids must be non-empty when use_user_group is disabled".to_string(),
                );
            }
            self.validate_api_key_group_selection(&owner_group_id, &group_ids, is_admin)
                .await?;
            group_ids
        };
        let id = uuid::Uuid::new_v4().to_string();
        let key = format!("sk-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let key_prefix = key[..12].to_string();
        let now = Utc::now();
        let expires_at = input
            .expires_in_days
            .map(|days| now + chrono::Duration::days(days));

        let model_limits_json =
            serde_json::to_string(&input.model_limits).map_err(|e| e.to_string())?;
        let ip_whitelist_json =
            serde_json::to_string(&input.ip_whitelist).map_err(|e| e.to_string())?;
        let group_ids_json = serialize_group_ids_json(&group_ids)?;
        let model_redirects_json =
            serde_json::to_string(&input.model_redirects).map_err(|e| e.to_string())?;

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        self.lock_user_balance_tx(&tx, user_id)
            .await
            .map_err(|e| e.message)?;
        tx.execute(self.db.stmt(
                r#"INSERT INTO api_keys (id, user_id, name, key_prefix, key, created_at, expires_at, enabled, sub_account_enabled, sub_account_balance_nano, model_limits_enabled, model_limits, ip_whitelist, use_user_group, group_ids, max_multiplier, transforms, model_redirects, reasoning_envelope_enabled, request_capture_enabled, request_capture_mode, request_capture_retention)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)"#,
                vec![
                    id.clone().into(),
                    user_id.into(),
                    input.name.clone().into(),
                    key_prefix.clone().into(),
                    key.clone().into(),
                    now.to_rfc3339().into(),
                    expires_at.map(|e| e.to_rfc3339()).into(),
                    SeaValue::Int(Some(if input.sub_account_enabled { 1 } else { 0 })),
                    initial_sub_account_balance.to_string().into(),
                    SeaValue::Int(Some(if input.model_limits_enabled { 1 } else { 0 })),
                    model_limits_json.into(),
                    ip_whitelist_json.into(),
                    SeaValue::Int(Some(if input.use_user_group { 1 } else { 0 })),
                    group_ids_json.into(),
                    input.max_multiplier.map(|v| v.to_string()).into(),
                    serde_json::to_string(&input.transforms).map_err(|e| e.to_string())?.into(),
                    model_redirects_json.into(),
                    SeaValue::Int(Some(if input.reasoning_envelope_enabled { 1 } else { 0 })),
                    SeaValue::Int(Some(if input.request_capture_mode.should_start_capture() {
                        1
                    } else {
                        0
                    })),
                    input.request_capture_mode.as_str().into(),
                    input.request_capture_retention.as_str().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if initial_sub_account_balance != 0 {
            self.insert_billing_ledger_tx(
                &tx,
                BillingLedgerEntry {
                    user_id,
                    kind: "admin_sub_account_adjustment",
                    delta_nano_usd: initial_sub_account_balance,
                    balance_after_nano_usd: Some(initial_sub_account_balance),
                    meta: &serde_json::json!({ "api_key_id": id, "initial": true }),
                    created_at_rfc3339: &now.to_rfc3339(),
                },
            )
            .await
            .map_err(|e| e.message)?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        let api_key = ApiKey {
            id,
            user_id: user_id.to_string(),
            name: input.name,
            key_prefix,
            key: key.clone(),
            created_at: now,
            expires_at,
            last_used_at: None,
            enabled: true,
            sub_account_enabled: input.sub_account_enabled,
            sub_account_balance_nano: initial_sub_account_balance.to_string(),
            model_limits_enabled: input.model_limits_enabled,
            model_limits: input.model_limits,
            ip_whitelist: input.ip_whitelist,
            use_user_group: input.use_user_group,
            group_ids,
            max_multiplier: input.max_multiplier,
            transforms: input.transforms,
            model_redirects: input.model_redirects,
            compiled_model_redirects,
            reasoning_envelope_enabled: input.reasoning_envelope_enabled,
            request_capture_mode: input.request_capture_mode,
            request_capture_retention: input.request_capture_retention,
        };

        Ok((api_key, key))
    }

    pub async fn create_api_key_extended_with_limit(
        &self,
        user_id: &str,
        input: CreateApiKeyInput,
        is_admin: bool,
        max_per_user: i64,
    ) -> Result<(ApiKey, String), CreateApiKeyWithLimitError> {
        if max_per_user <= 0 {
            return Err(CreateApiKeyWithLimitError::InvalidRequest(
                "api_key_max_per_user must be positive".to_string(),
            ));
        }
        let _creation_guard = self.api_key_creation_lock.lock().await;
        let count = self
            .count_user_api_keys(user_id)
            .await
            .map_err(CreateApiKeyWithLimitError::InvalidRequest)?;
        if count >= max_per_user {
            return Err(CreateApiKeyWithLimitError::LimitReached {
                limit: max_per_user,
            });
        }
        self.create_api_key_extended(user_id, input, is_admin)
            .await
            .map_err(CreateApiKeyWithLimitError::InvalidRequest)
    }

    pub async fn get_api_key_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.created_at, a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled, a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits, a.ip_whitelist, a.use_user_group, a.group_ids, a.max_multiplier, a.transforms, a.model_redirects, a.reasoning_envelope_enabled, a.request_capture_enabled, a.request_capture_mode, a.request_capture_retention, u.role AS owner_role FROM api_keys a JOIN users u ON u.id = a.user_id WHERE a.key_prefix = $1",
                vec![prefix.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_api_key(&row).await?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_api_key_by_key(&self, key: &str) -> Result<Option<ApiKey>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.created_at, a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled, a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits, a.ip_whitelist, a.use_user_group, a.group_ids, a.max_multiplier, a.transforms, a.model_redirects, a.reasoning_envelope_enabled, a.request_capture_enabled, a.request_capture_mode, a.request_capture_retention, u.role AS owner_role FROM api_keys a JOIN users u ON u.id = a.user_id WHERE a.key = $1",
                vec![key.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        match row {
            Some(row) => Ok(Some(self.row_to_api_key(&row).await?)),
            None => Ok(None),
        }
    }

    pub(super) async fn get_api_key_auth_candidate(
        &self,
        key: &str,
    ) -> Result<Option<(ApiKey, User)>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key,
                        a.created_at, a.expires_at, a.last_used_at, a.enabled,
                        a.sub_account_enabled, a.sub_account_balance_nano,
                        a.model_limits_enabled, a.model_limits, a.ip_whitelist,
                        a.use_user_group, a.group_ids, a.max_multiplier, a.transforms,
                        a.model_redirects, a.reasoning_envelope_enabled,
                        a.request_capture_enabled, a.request_capture_mode,
                        a.request_capture_retention,
                        u.role AS owner_role,
                        u.id AS owner_id, u.username AS owner_username,
                        u.password_hash AS owner_password_hash,
                        u.created_at AS owner_created_at, u.updated_at AS owner_updated_at,
                        u.last_login_at AS owner_last_login_at, u.enabled AS owner_enabled,
                        u.balance_nano_usd AS owner_balance_nano_usd,
                        u.balance_unlimited AS owner_balance_unlimited,
                        u.email AS owner_email, u.group_id AS owner_group_id
                  FROM api_keys a
                  JOIN users u ON u.id = a.user_id
                  WHERE a.key = $1
                  LIMIT 1",
                vec![key.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let api_key = self.row_to_api_key(&row).await?;
        let role_raw: String = row.try_get("", "owner_role").map_err(|e| e.to_string())?;
        let role = UserRole::from_str(&role_raw).ok_or_else(|| "invalid role".to_string())?;
        let parse_time = |column: &str| -> Result<DateTime<Utc>, String> {
            DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", column)
                    .map_err(|e| e.to_string())?,
            )
            .map(|value| value.with_timezone(&Utc))
            .map_err(|e| e.to_string())
        };
        let last_login_at = row
            .try_get::<Option<String>>("", "owner_last_login_at")
            .map_err(|e| e.to_string())?
            .map(|value| DateTime::parse_from_rfc3339(&value).map(|v| v.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;
        let balance_nano_usd: String = row
            .try_get("", "owner_balance_nano_usd")
            .map_err(|e| e.to_string())?;
        parse_nano_usd(&balance_nano_usd)
            .map_err(|e| format!("invalid persisted user balance: {e}"))?;
        let owner_enabled = decode_required_bool(&row, "owner_enabled")?;
        let owner_group_id: String = row
            .try_get("", "owner_group_id")
            .map_err(|error| format!("invalid persisted users.group_id: {error}"))?;
        let user = User {
            id: row.try_get("", "owner_id").map_err(|e| e.to_string())?,
            username: row
                .try_get("", "owner_username")
                .map_err(|e| e.to_string())?,
            password_hash: row
                .try_get("", "owner_password_hash")
                .map_err(|e| e.to_string())?,
            role,
            created_at: parse_time("owner_created_at")?,
            updated_at: parse_time("owner_updated_at")?,
            last_login_at,
            enabled: owner_enabled,
            balance_nano_usd,
            balance_unlimited: row
                .try_get::<i32>("", "owner_balance_unlimited")
                .map_err(|e| e.to_string())?
                == 1,
            email: row
                .try_get::<Option<String>>("", "owner_email")
                .map_err(|e| e.to_string())?,
            group_id: owner_group_id,
        };
        Ok(Some((api_key, user)))
    }

    pub async fn list_user_api_keys(&self, user_id: &str) -> Result<Vec<ApiKey>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.created_at,
                        a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled,
                        a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits,
                        a.ip_whitelist, a.use_user_group, a.group_ids, a.max_multiplier,
                        a.transforms, a.model_redirects, a.reasoning_envelope_enabled,
                        a.request_capture_enabled, a.request_capture_mode, a.request_capture_retention, u.role AS owner_role
                 FROM api_keys a JOIN users u ON u.id = a.user_id
                 WHERE a.user_id = $1 ORDER BY a.created_at DESC",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        let mut api_keys = Vec::with_capacity(rows.len());
        for row in &rows {
            api_keys.push(self.row_to_api_key(row).await?);
        }
        Ok(api_keys)
    }

    pub async fn count_user_api_keys(&self, user_id: &str) -> Result<i64, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT COUNT(*) AS cnt FROM api_keys WHERE user_id = $1",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "count query returned no row".to_string())?;
        row.try_get("", "cnt").map_err(|e| e.to_string())
    }

    pub async fn get_api_key_for_user(
        &self,
        id: &str,
        user_id: &str,
    ) -> Result<Option<ApiKey>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.created_at,
                        a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled,
                        a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits,
                        a.ip_whitelist, a.use_user_group, a.group_ids, a.max_multiplier,
                        a.transforms, a.model_redirects, a.reasoning_envelope_enabled,
                        a.request_capture_enabled, a.request_capture_mode, a.request_capture_retention, u.role AS owner_role
                 FROM api_keys a JOIN users u ON u.id = a.user_id
                 WHERE a.id = $1 AND a.user_id = $2",
                vec![id.into(), user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        match row {
            Some(row) => Ok(Some(self.row_to_api_key(&row).await?)),
            None => Ok(None),
        }
    }

    pub async fn filter_user_api_key_ids(
        &self,
        user_id: &str,
        ids: &[String],
    ) -> Result<Vec<String>, String> {
        if ids.len() > api_key_batch_delete_max_ids() {
            return Err(format!(
                "batch delete accepts at most {} ids",
                api_key_batch_delete_max_ids()
            ));
        }
        let mut ids = ids.to_vec();
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..ids.len())
            .map(|index| format!("${}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let mut values: Vec<SeaValue> = Vec::with_capacity(ids.len() + 1);
        values.push(user_id.into());
        values.extend(ids.iter().cloned().map(Into::into));
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id FROM api_keys WHERE user_id = $1 AND id IN ({placeholders}) ORDER BY id"
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.into_iter()
            .map(|row| row.try_get("", "id").map_err(|e| e.to_string()))
            .collect()
    }

    pub async fn update_api_key_last_used(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "UPDATE api_keys SET last_used_at = $1 WHERE id = $2",
                vec![now.to_rfc3339().into(), id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn validate_api_key(&self, key: &str) -> Result<Option<(ApiKey, User)>, String> {
        if key.len() < 12 || key.len() > MAX_FORWARDING_API_KEY_BYTES {
            return Ok(None);
        }

        loop {
            if let Some((cached_key, cached_user)) = self.api_key_cache.get(key) {
                let now = Utc::now();
                let not_expired = cached_key
                    .expires_at
                    .is_none_or(|expires_at| expires_at >= now);
                let is_valid = cached_key.enabled
                    && cached_user.enabled
                    && not_expired
                    && key == cached_key.key;
                if is_valid {
                    self.last_used_batcher.record(cached_key.id.clone(), now);
                    return Ok(Some((cached_key, cached_user)));
                }

                self.api_key_cache.invalidate(key);
            }

            let generation = self.api_key_cache.current_generation();
            let (api_key, user) = match self.get_api_key_auth_candidate(key).await? {
                Some(candidate) => candidate,
                None => return Ok(None),
            };

            if !api_key.enabled {
                return Ok(None);
            }

            if let Some(expires_at) = api_key.expires_at
                && expires_at < Utc::now()
            {
                return Ok(None);
            }

            if key != api_key.key {
                return Ok(None);
            }

            if !user.enabled {
                return Ok(None);
            }

            if !self.api_key_cache.insert_if_current(
                key.to_string(),
                generation,
                api_key.clone(),
                user.clone(),
            ) {
                continue;
            }

            self.last_used_batcher
                .record(api_key.id.clone(), Utc::now());

            return Ok(Some((api_key, user)));
        }
    }

    /// Get API key by ID
    pub async fn get_api_key_by_id(&self, id: &str) -> Result<Option<ApiKey>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.created_at, a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled, a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits, a.ip_whitelist, a.use_user_group, a.group_ids, a.max_multiplier, a.transforms, a.model_redirects, a.reasoning_envelope_enabled, a.request_capture_enabled, a.request_capture_mode, a.request_capture_retention, u.role AS owner_role FROM api_keys a JOIN users u ON u.id = a.user_id WHERE a.id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_api_key(&row).await?))
        } else {
            Ok(None)
        }
    }

    /// Batch delete API keys
    pub async fn batch_delete_api_keys(&self, ids: &[String]) -> Result<usize, String> {
        self.delete_api_keys_transactional(ids).await
    }

    pub async fn delete_api_key(&self, id: &str) -> Result<(), String> {
        self.delete_api_keys_transactional(&[id.to_string()])
            .await?;
        Ok(())
    }

    pub(crate) async fn row_to_api_key(&self, row: &QueryResult) -> Result<ApiKey, String> {
        let expires_at: Option<String> =
            row.try_get("", "expires_at").map_err(|e| e.to_string())?;
        let expires_at = expires_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        let last_used_at: Option<String> =
            row.try_get("", "last_used_at").map_err(|e| e.to_string())?;
        let last_used_at = last_used_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;

        let sub_account_enabled = decode_required_bool(row, "sub_account_enabled")?;
        let sub_account_balance_nano: String = row
            .try_get("", "sub_account_balance_nano")
            .map_err(|e| e.to_string())?;
        parse_nano_usd(&sub_account_balance_nano)
            .map_err(|e| format!("invalid persisted sub-account balance: {e}"))?;
        let model_limits_enabled = decode_required_bool(row, "model_limits_enabled")?;

        let model_limits_str: String = row
            .try_get("", "model_limits")
            .map_err(|error| format!("invalid persisted model_limits: {error}"))?;
        let model_limits = parse_persisted_json_array(&model_limits_str, "model_limits")?;

        let ip_whitelist_str: String = row
            .try_get("", "ip_whitelist")
            .map_err(|error| format!("invalid persisted ip_whitelist: {error}"))?;
        let ip_whitelist: Vec<String> =
            parse_persisted_json_array(&ip_whitelist_str, "ip_whitelist")?;
        let ip_whitelist = canonicalize_ip_whitelist(&ip_whitelist)
            .map_err(|error| format!("invalid persisted ip_whitelist: {error}"))?;
        let use_user_group = decode_required_bool(row, "use_user_group")?;
        let group_ids_raw = row
            .try_get::<Option<String>>("", "group_ids")
            .map_err(|error| format!("invalid persisted api_keys.group_ids: {error}"))?;
        let group_ids = parse_group_ids_json(group_ids_raw.as_deref(), "api_keys.group_ids")?;

        let max_multiplier = row
            .try_get::<Option<String>>("", "max_multiplier")
            .map_err(|e| e.to_string())?
            .map(|value| value.parse())
            .transpose()
            .map_err(|e: String| format!("invalid persisted max_multiplier: {e}"))?;
        validate_positive_max_multiplier(max_multiplier)
            .map_err(|error| format!("invalid persisted max_multiplier: {error}"))?;
        let transforms_str: String = row
            .try_get("", "transforms")
            .map_err(|error| format!("invalid persisted transforms: {error}"))?;
        let model_redirects_str: String = row
            .try_get("", "model_redirects")
            .map_err(|error| format!("invalid persisted model_redirects: {error}"))?;
        let user_id: String = row.try_get("", "user_id").map_err(|e| e.to_string())?;
        let owner_role = row
            .try_get::<String>("", "owner_role")
            .map_err(|error| format!("invalid persisted owner_role: {error}"))?;
        let is_admin = UserRole::from_str(&owner_role)
            .ok_or_else(|| format!("invalid persisted owner_role: {owner_role:?}"))?
            .can_manage_system();
        let transforms = parse_persisted_json_array(&transforms_str, "transforms")?;
        let transforms: Vec<TransformRuleConfig> =
            sanitize_api_key_transforms(transforms, is_admin, &self.custom_transforms.get());
        let model_redirects: Vec<ModelRedirectRule> =
            parse_persisted_json_array(&model_redirects_str, "model_redirects")?;
        let compiled_model_redirects = compile_model_redirects(&model_redirects)
            .map_err(|error| format!("invalid persisted model_redirects: {error}"))?;
        let reasoning_envelope_enabled = decode_required_bool(row, "reasoning_envelope_enabled")?;
        let request_capture_mode = decode_request_capture_mode(row)?;
        let request_capture_retention = decode_request_capture_retention(row)?;

        Ok(ApiKey {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            user_id,
            name: row.try_get("", "name").map_err(|e| e.to_string())?,
            key_prefix: row.try_get("", "key_prefix").map_err(|e| e.to_string())?,
            key: row.try_get("", "key").map_err(|e| e.to_string())?,
            created_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", "created_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            expires_at,
            last_used_at,
            enabled: decode_required_bool(row, "enabled")?,
            sub_account_enabled,
            sub_account_balance_nano,
            model_limits_enabled,
            model_limits,
            ip_whitelist,
            use_user_group,
            group_ids,
            max_multiplier,
            transforms,
            model_redirects,
            compiled_model_redirects,
            reasoning_envelope_enabled,
            request_capture_mode,
            request_capture_retention,
        })
    }
}

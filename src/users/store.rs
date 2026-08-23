use super::utils::parse_nano_usd;
use super::{
    AdminUpdateUserInput, ApiKey, BillingError, BillingErrorKind, CreateApiKeyInput,
    CreateApiKeyWithLimitError, ModelRedirectRule, RESERVED_INTERNAL_USER_PREFIX,
    RegisterUserError, RequestCaptureMode, Session, UpdateApiKeyInput, User, UserBalance, UserRole,
    UserStore, canonicalize_groups, validate_model_redirects,
};
use crate::transforms::{
    TransformRuleConfig, canonical_transform_id, canonicalize_transform_rule,
    canonicalize_transform_rules,
};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use sea_orm::Value as SeaValue;
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

const MAX_FORWARDING_API_KEY_BYTES: usize = 512;
const DEFAULT_API_KEY_BATCH_DELETE_MAX_IDS: usize = 400;
const DEFAULT_SESSION_CLEANUP_INTERVAL_SECS: u64 = 3_600;

fn parse_positive_limit(raw: Option<&str>, default: usize) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_api_key_batch_delete_limit(raw: Option<&str>) -> usize {
    parse_positive_limit(raw, DEFAULT_API_KEY_BATCH_DELETE_MAX_IDS)
        .min(DEFAULT_API_KEY_BATCH_DELETE_MAX_IDS)
}

fn api_key_batch_delete_max_ids() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        parse_api_key_batch_delete_limit(
            std::env::var("MONOIZE_API_KEY_BATCH_DELETE_MAX_IDS")
                .ok()
                .as_deref(),
        )
    })
}

fn parse_session_cleanup_interval_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SESSION_CLEANUP_INTERVAL_SECS)
}

fn session_cleanup_interval() -> std::time::Duration {
    static INTERVAL: OnceLock<std::time::Duration> = OnceLock::new();
    *INTERVAL.get_or_init(|| {
        std::time::Duration::from_secs(parse_session_cleanup_interval_secs(
            std::env::var("MONOIZE_SESSION_CLEANUP_INTERVAL_SECONDS")
                .ok()
                .as_deref(),
        ))
    })
}

fn api_key_lookup_hash(key: &str) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(key.as_bytes()))
}

fn canonicalize_ip_whitelist(entries: &[String]) -> Result<Vec<String>, String> {
    let mut canonical = BTreeSet::new();
    for entry in entries {
        let value = entry.trim();
        if value.is_empty() {
            return Err("ip_whitelist entries must not be empty".to_string());
        }
        let normalized = if let Ok(ip) = value.parse::<IpAddr>() {
            ip.to_string()
        } else if let Ok(network) = value.parse::<ipnet::IpNet>() {
            network.to_string()
        } else {
            return Err(format!("invalid ip_whitelist entry: {value}"));
        };
        canonical.insert(normalized);
    }
    Ok(canonical.into_iter().collect())
}

const ALLOWED_API_KEY_REQUEST_TRANSFORMS: &[&str] = &[
    "inject_system_prompt",
    "system_to_developer_role",
    "merge_consecutive_roles",
    "append_empty_user_message",
    "compress_user_message_images",
    "enable_openai_image_generation_tool",
    "strip_anthropic_billing_header",
    "auto_cache_system",
    "auto_cache_tool_use",
    "auto_cache_openai_tool_use",
    "auto_cache_user_id",
    "auto_cache_openai_prompt",
];

const ALLOWED_API_KEY_RESPONSE_TRANSFORMS: &[&str] = &[
    "strip_reasoning",
    "strip_encrypted_reasoning",
    "reasoning_to_think_xml",
    "think_xml_to_reasoning",
    "split_sse_frames",
    "plaintext_reasoning_to_summary",
    "reasoning_content_delta",
    "reasoning_summary_to_raw_cot",
    "assistant_markdown_images_to_output",
    "assistant_output_images_to_markdown",
    "compress_assistant_output_images",
];

#[derive(Clone, Copy)]
struct LockedUserBalance {
    balance: i128,
    unlimited: bool,
}

struct LockedApiKeyBalance {
    user_id: String,
    balance: i128,
    sub_account_enabled: bool,
}

pub(crate) fn is_allowed_api_key_transform(rule: &TransformRuleConfig) -> bool {
    let transform = canonical_transform_id(rule.transform.as_str());
    match rule.phase {
        crate::transforms::Phase::Request => {
            ALLOWED_API_KEY_REQUEST_TRANSFORMS.contains(&transform)
        }
        crate::transforms::Phase::Response => {
            ALLOWED_API_KEY_RESPONSE_TRANSFORMS.contains(&transform)
        }
    }
}

pub(crate) fn sanitize_api_key_transforms(
    transforms: Vec<TransformRuleConfig>,
    is_admin: bool,
) -> Vec<TransformRuleConfig> {
    let transforms: Vec<TransformRuleConfig> = transforms
        .into_iter()
        .map(|mut rule| {
            canonicalize_transform_rule(&mut rule);
            rule
        })
        .collect();
    if is_admin {
        return transforms;
    }
    transforms
        .into_iter()
        .filter(is_allowed_api_key_transform)
        .collect()
}

pub(crate) fn validate_api_key_transforms(
    transforms: &[TransformRuleConfig],
    is_admin: bool,
) -> Result<(), String> {
    if is_admin {
        return Ok(());
    }
    for rule in transforms {
        let mut canonical_rule = rule.clone();
        canonicalize_transform_rule(&mut canonical_rule);
        if !is_allowed_api_key_transform(&canonical_rule) {
            return Err(format!(
                "transform '{}' is not allowed for API keys",
                rule.transform
            ));
        }
    }
    Ok(())
}

fn parse_persisted_json_array<T>(raw: &str, column: &str) -> Result<Vec<T>, String>
where
    T: DeserializeOwned,
{
    serde_json::from_str(raw).map_err(|error| format!("invalid persisted {column}: {error}"))
}

pub(crate) fn parse_allowed_groups_json(
    raw: Option<&str>,
    column: &str,
) -> Result<Vec<String>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let groups = serde_json::from_str::<Option<Vec<String>>>(raw)
        .map_err(|error| format!("invalid persisted {column}: {error}"))?
        .unwrap_or_default();
    Ok(canonicalize_groups(&groups))
}

pub(crate) fn decode_required_bool(row: &QueryResult, column: &str) -> Result<bool, String> {
    let value = row
        .try_get::<i32>("", column)
        .map_err(|error| format!("invalid persisted {column}: {error}"))?;
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!(
            "invalid persisted {column}: expected integer 0 or 1, got {value}"
        )),
    }
}

fn decode_request_capture_mode(row: &QueryResult) -> Result<RequestCaptureMode, String> {
    let raw = row
        .try_get::<Option<String>>("", "request_capture_mode")
        .map_err(|error| format!("invalid persisted request_capture_mode: {error}"))?;
    match raw.as_deref().map(str::trim) {
        None => Ok(RequestCaptureMode::Off),
        Some("off") => Ok(RequestCaptureMode::Off),
        Some("capture-all") => Ok(RequestCaptureMode::CaptureAll),
        Some("capture-only-abnormal") => Ok(RequestCaptureMode::CaptureOnlyAbnormal),
        Some(value) => Err(format!(
            "invalid persisted request_capture_mode: unsupported value {value:?}"
        )),
    }
}

impl UserStore {
    pub fn api_key_batch_delete_max_ids() -> usize {
        api_key_batch_delete_max_ids()
    }
}

pub(crate) fn serialize_allowed_groups_json(groups: &[String]) -> Result<String, String> {
    serde_json::to_string(&canonicalize_groups(groups)).map_err(|e| e.to_string())
}

fn validate_api_key_allowed_groups_subset(
    user_groups: &[String],
    key_groups: &[String],
) -> Result<(), String> {
    let user_groups = canonicalize_groups(user_groups);
    if user_groups.is_empty() {
        return Ok(());
    }

    let user_groups: BTreeSet<_> = user_groups.into_iter().collect();
    let key_groups = canonicalize_groups(key_groups);
    if key_groups.iter().all(|group| user_groups.contains(group)) {
        Ok(())
    } else {
        Err(
            "invalid_request: api key allowed_groups must be a subset of the owning user's allowed_groups"
                .to_string(),
        )
    }
}

impl UserStore {
    pub fn is_reserved_internal_username(username: &str) -> bool {
        username
            .trim()
            .to_ascii_lowercase()
            .starts_with(RESERVED_INTERNAL_USER_PREFIX)
    }

    pub async fn new(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<super::InsertRequestLog>>,
    ) -> Result<Self, String> {
        Self::new_with_pending_request_logs(db, log_broadcast, Arc::new(dashmap::DashMap::new()))
            .await
    }

    pub async fn new_with_pending_request_logs(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<super::InsertRequestLog>>,
        pending_request_logs: Arc<dashmap::DashMap<String, super::InsertRequestLog>>,
    ) -> Result<Self, String> {
        Self::new_with_pending_request_logs_and_spool_dir(
            db,
            log_broadcast,
            pending_request_logs,
            None,
        )
        .await
    }

    pub async fn new_with_pending_request_logs_and_spool_dir(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<super::InsertRequestLog>>,
        pending_request_logs: Arc<dashmap::DashMap<String, super::InsertRequestLog>>,
        request_log_spool_dir: Option<std::path::PathBuf>,
    ) -> Result<Self, String> {
        use std::time::Duration;
        let store = Self {
            db,
            last_used_batcher: crate::db_cache::LastUsedBatcher::new(),
            request_log_batcher: crate::db_cache::RequestLogBatcher::new_with_spool_dir(
                128,
                request_log_spool_dir,
                log_broadcast,
                pending_request_logs,
            ),
            api_key_cache: crate::db_cache::ApiKeyCache::new(Duration::from_secs(60)),
            balance_cache: crate::db_cache::BalanceCache::new(Duration::from_secs(30)),
            registration_lock: Arc::new(tokio::sync::Mutex::new(())),
            api_key_creation_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        store.migrate_transform_rule_ids().await?;
        store.migrate_api_key_lookup_hashes().await?;
        store.cleanup_expired_sessions().await?;
        Ok(store)
    }

    async fn migrate_api_key_lookup_hashes(&self) -> Result<(), String> {
        const HASH_BACKFILL_CHUNK_SIZE: usize = 300;
        let mut cursor: Option<String> = None;
        loop {
            let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
            let rows = match cursor.as_deref() {
                Some(cursor) => {
                    tx.query_all(self.db.stmt(
                        &format!(
                            "SELECT id, key, key_hash FROM api_keys
                             WHERE id > $1
                             ORDER BY id LIMIT {HASH_BACKFILL_CHUNK_SIZE}"
                        ),
                        vec![cursor.into()],
                    ))
                    .await
                }
                None => {
                    tx.query_all(self.db.stmt(
                        &format!(
                            "SELECT id, key, key_hash FROM api_keys
                             ORDER BY id LIMIT {HASH_BACKFILL_CHUNK_SIZE}"
                        ),
                        vec![],
                    ))
                    .await
                }
            }
            .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                tx.commit().await.map_err(|e| e.to_string())?;
                break;
            }
            let mut updates = Vec::with_capacity(rows.len());
            for row in rows {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                let key: String = row.try_get("", "key").map_err(|e| e.to_string())?;
                cursor = Some(id.clone());
                let stored_hash: Option<String> =
                    row.try_get("", "key_hash").map_err(|e| e.to_string())?;
                let expected_hash = api_key_lookup_hash(&key);
                if stored_hash.as_deref() != Some(expected_hash.as_str()) {
                    updates.push((id, expected_hash));
                }
            }
            if updates.is_empty() {
                tx.commit().await.map_err(|e| e.to_string())?;
                continue;
            }
            let mut values: Vec<SeaValue> = Vec::with_capacity(updates.len() * 3);
            let mut cases = Vec::with_capacity(updates.len());
            let mut ids = Vec::with_capacity(updates.len());
            for (id, key_hash) in &updates {
                let id_index = values.len() + 1;
                values.push(id.clone().into());
                let hash_index = values.len() + 1;
                values.push(key_hash.clone().into());
                cases.push(format!("WHEN ${id_index} THEN ${hash_index}"));
            }
            for (id, _) in &updates {
                let id_index = values.len() + 1;
                values.push(id.clone().into());
                ids.push(format!("${id_index}"));
            }
            tx.execute(self.db.stmt(
                &format!(
                    "UPDATE api_keys
                     SET key_hash = CASE id {} ELSE key_hash END
                     WHERE id IN ({})",
                    cases.join(" "),
                    ids.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn migrate_transform_rule_ids(&self) -> Result<(), String> {
        const TRANSFORM_MIGRATION_MARKER: &str = "migration.api_key_transform_rule_ids.v1";
        const TRANSFORM_MIGRATION_CHUNK_SIZE: usize = 300;
        let marker = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT value FROM system_settings WHERE key = $1",
                vec![TRANSFORM_MIGRATION_MARKER.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if marker
            .as_ref()
            .and_then(|row| row.try_get::<String>("", "value").ok())
            .as_deref()
            == Some("complete")
        {
            return Ok(());
        }
        let mut cursor: Option<String> = None;
        loop {
            let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
            let rows = match cursor.as_deref() {
                Some(cursor) => {
                    tx.query_all(self.db.stmt(
                        &format!(
                            "SELECT id, transforms FROM api_keys
                             WHERE id > $1 ORDER BY id LIMIT {TRANSFORM_MIGRATION_CHUNK_SIZE}"
                        ),
                        vec![cursor.into()],
                    ))
                    .await
                }
                None => {
                    tx.query_all(self.db.stmt(
                        &format!(
                            "SELECT id, transforms FROM api_keys
                             ORDER BY id LIMIT {TRANSFORM_MIGRATION_CHUNK_SIZE}"
                        ),
                        vec![],
                    ))
                    .await
                }
            }
            .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                tx.commit().await.map_err(|e| e.to_string())?;
                break;
            }

            let mut updates = Vec::new();
            for row in rows {
                let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                cursor = Some(id.clone());
                let raw: String = row
                    .try_get("", "transforms")
                    .unwrap_or_else(|_| "[]".to_string());
                let Ok(mut transforms) = serde_json::from_str::<Vec<TransformRuleConfig>>(&raw)
                else {
                    tracing::warn!(api_key_id = %id, "skip invalid api key transforms during transform id migration");
                    continue;
                };
                if !canonicalize_transform_rules(&mut transforms) {
                    continue;
                }
                let encoded = serde_json::to_string(&transforms).map_err(|e| e.to_string())?;
                updates.push((id, encoded));
            }
            if !updates.is_empty() {
                let mut values: Vec<SeaValue> = Vec::with_capacity(updates.len() * 3);
                let mut cases = Vec::with_capacity(updates.len());
                let mut ids = Vec::with_capacity(updates.len());
                for (id, transforms) in &updates {
                    let id_index = values.len() + 1;
                    values.push(id.clone().into());
                    let transforms_index = values.len() + 1;
                    values.push(transforms.clone().into());
                    cases.push(format!("WHEN ${id_index} THEN ${transforms_index}"));
                }
                for (id, _) in &updates {
                    let id_index = values.len() + 1;
                    values.push(id.clone().into());
                    ids.push(format!("${id_index}"));
                }
                tx.execute(self.db.stmt(
                    &format!(
                        "UPDATE api_keys SET transforms = CASE id {} ELSE transforms END
                         WHERE id IN ({})",
                        cases.join(" "),
                        ids.join(", ")
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
            }
            tx.commit().await.map_err(|e| e.to_string())?;
            for (id, _) in updates {
                self.api_key_cache.invalidate_by_key_id(&id);
            }
        }
        let tx = self.db.begin_write().await.map_err(|e| e.to_string())?;
        tx.execute(self.db.stmt(
            "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            vec![
                TRANSFORM_MIGRATION_MARKER.into(),
                "complete".into(),
                Utc::now().to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())
    }

    pub fn spawn_background_tasks(&self) {
        self.last_used_batcher
            .clone()
            .spawn_flush_task(self.db.clone(), std::time::Duration::from_secs(30));
        self.request_log_batcher
            .clone()
            .spawn_flush_task(self.db.clone(), std::time::Duration::from_secs(2));
        self.api_key_cache
            .clone()
            .spawn_eviction_task(std::time::Duration::from_secs(30));
        self.balance_cache
            .clone()
            .spawn_eviction_task(std::time::Duration::from_secs(30));
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(session_cleanup_interval()).await;
                if let Err(error) = store.cleanup_expired_sessions().await {
                    tracing::warn!(%error, "failed to cleanup expired sessions");
                }
            }
        });
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    super::request_logs::REQUEST_LOG_RETENTION_INTERVAL_SECS,
                ))
                .await;
                if let Err(e) = store.cleanup_expired_request_logs().await {
                    tracing::warn!("failed to cleanup expired request logs: {e}");
                }
            }
        });
        self.spawn_plan_grant_scheduler();
    }

    pub async fn flush_all_batchers(&self) {
        self.last_used_batcher.flush(&self.db).await;
        self.request_log_batcher.flush(&self.db).await;
    }

    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    }

    pub fn verify_password(password: &str, hash: &str) -> Result<bool, String> {
        let parsed_hash = PasswordHash::new(hash).map_err(|e| e.to_string())?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    pub async fn user_count(&self) -> Result<i64, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT COUNT(*) as count FROM users WHERE substr(lower(username), 1, 9) != '_monoize_'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let row = row.ok_or_else(|| "no count row".to_string())?;
        row.try_get::<i64>("", "count").map_err(|e| e.to_string())
    }

    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        role: UserRole,
        allowed_groups: &[String],
    ) -> Result<User, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let password_hash = Self::hash_password(password)?;
        let now = Utc::now();
        let allowed_groups = canonicalize_groups(allowed_groups);
        let allowed_groups_json = serialize_allowed_groups_json(&allowed_groups)?;

        self.db.write().await
            .execute(self.db.stmt(
                r#"INSERT INTO users (id, username, password_hash, role, created_at, updated_at, enabled, balance_nano_usd, balance_unlimited, allowed_groups)
                   VALUES ($1, $2, $3, $4, $5, $6, 1, '0', 0, $7)"#,
                vec![
                    id.clone().into(),
                    username.into(),
                    password_hash.clone().into(),
                    role.as_str().into(),
                    now.to_rfc3339().into(),
                    now.to_rfc3339().into(),
                    allowed_groups_json.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(User {
            id,
            username: username.to_string(),
            password_hash,
            role,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            enabled: true,
            balance_nano_usd: "0".to_string(),
            balance_unlimited: false,
            email: None,
            allowed_groups,
            billing_plan_id: None,
            next_grant_at: None,
        })
    }

    pub async fn register_user_atomic(
        &self,
        username: &str,
        password: &str,
        registration_enabled: bool,
    ) -> Result<User, RegisterUserError> {
        let _registration_guard = self.registration_lock.lock().await;
        let user_count = self
            .user_count()
            .await
            .map_err(RegisterUserError::Storage)?;
        if user_count != 0 && !registration_enabled {
            return Err(RegisterUserError::RegistrationDisabled);
        }
        if self
            .get_user_by_username(username)
            .await
            .map_err(RegisterUserError::Storage)?
            .is_some()
        {
            return Err(RegisterUserError::UsernameExists);
        }
        let role = if user_count == 0 {
            UserRole::SuperAdmin
        } else {
            UserRole::User
        };
        self.create_user(username, password, role, &[])
            .await
            .map_err(RegisterUserError::Storage)
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email, allowed_groups, billing_plan_id, next_grant_at FROM users WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_user(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email, allowed_groups, billing_plan_id, next_grant_at FROM users WHERE username = $1",
                vec![username.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            Ok(Some(self.row_to_user(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_users(&self) -> Result<Vec<User>, String> {
        let rows = self.db.read()
            .query_all(self.db.stmt(
                "SELECT id, username, password_hash, role, created_at, updated_at, last_login_at, enabled, balance_nano_usd, balance_unlimited, email, allowed_groups, billing_plan_id, next_grant_at FROM users WHERE substr(lower(username), 1, 9) != '_monoize_' ORDER BY created_at DESC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter().map(|row| self.row_to_user(row)).collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_user(
        &self,
        id: &str,
        username: Option<&str>,
        password: Option<&str>,
        role: Option<UserRole>,
        enabled: Option<bool>,
        balance_nano_usd: Option<&str>,
        balance_unlimited: Option<bool>,
        email: Option<Option<&str>>,
        allowed_groups: Option<&[String]>,
    ) -> Result<(), String> {
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(u) = username {
            set_clauses.push(format!("username = ${idx}"));
            values.push(u.into());
            idx += 1;
        }
        if let Some(p) = password {
            set_clauses.push(format!("password_hash = ${idx}"));
            values.push(Self::hash_password(p)?.into());
            idx += 1;
        }
        if let Some(r) = role {
            set_clauses.push(format!("role = ${idx}"));
            values.push(r.as_str().into());
            idx += 1;
        }
        if let Some(e) = enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if e { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(balance) = balance_nano_usd {
            parse_nano_usd(balance)?;
            set_clauses.push(format!("balance_nano_usd = ${idx}"));
            values.push(balance.into());
            idx += 1;
        }
        if let Some(unlimited) = balance_unlimited {
            set_clauses.push(format!("balance_unlimited = ${idx}"));
            values.push(SeaValue::Int(Some(if unlimited { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(email_opt) = email {
            match email_opt {
                Some(e) if !e.trim().is_empty() => {
                    set_clauses.push(format!("email = ${idx}"));
                    values.push(e.trim().into());
                    idx += 1;
                }
                _ => {
                    set_clauses.push("email = NULL".to_string());
                }
            }
        }
        if let Some(groups) = allowed_groups {
            set_clauses.push(format!("allowed_groups = ${idx}"));
            values.push(serialize_allowed_groups_json(groups)?.into());
            idx += 1;
        }
        if set_clauses.is_empty() {
            return Ok(());
        }

        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(Utc::now().to_rfc3339().into());
        idx += 1;

        values.push(id.into());

        let query = format!(
            "UPDATE users SET {} WHERE id = ${idx}",
            set_clauses.join(", ")
        );

        self.db
            .write()
            .await
            .execute(self.db.stmt(&query, values))
            .await
            .map_err(|e| e.to_string())?;

        if !set_clauses.is_empty() {
            self.api_key_cache.invalidate_by_user_id(id);
        }
        if balance_nano_usd.is_some() || balance_unlimited.is_some() {
            self.balance_cache.invalidate(id);
        }

        Ok(())
    }

    pub async fn admin_update_user_atomic(
        &self,
        id: &str,
        input: AdminUpdateUserInput,
        actor_user_id: &str,
    ) -> Result<(), String> {
        let AdminUpdateUserInput {
            username,
            password,
            role,
            enabled,
            balance_nano_usd,
            balance_unlimited,
            email,
            allowed_groups,
            billing_plan_id,
        } = input;
        let has_balance_change = balance_nano_usd.is_some() || balance_unlimited.is_some();
        let has_plan_change = billing_plan_id.is_some();
        if username.is_none()
            && password.is_none()
            && role.is_none()
            && enabled.is_none()
            && !has_balance_change
            && email.is_none()
            && allowed_groups.is_none()
            && billing_plan_id.is_none()
        {
            return Ok(());
        }

        let password_hash = password.as_deref().map(Self::hash_password).transpose()?;
        let parsed_balance = balance_nano_usd
            .as_deref()
            .map(parse_nano_usd)
            .transpose()?;
        let allowed_groups_json = allowed_groups
            .as_deref()
            .map(serialize_allowed_groups_json)
            .transpose()?;

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        let current = self
            .lock_user_balance_tx(&tx, id)
            .await
            .map_err(|error| error.message)?;
        let new_balance = parsed_balance.unwrap_or(current.balance);
        let new_unlimited = balance_unlimited.unwrap_or(current.unlimited);
        let now = Utc::now().to_rfc3339();
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(username) = username {
            set_clauses.push(format!("username = ${idx}"));
            values.push(username.into());
            idx += 1;
        }
        if let Some(password_hash) = password_hash {
            set_clauses.push(format!("password_hash = ${idx}"));
            values.push(password_hash.into());
            idx += 1;
        }
        if let Some(role) = role {
            set_clauses.push(format!("role = ${idx}"));
            values.push(role.as_str().into());
            idx += 1;
        }
        if let Some(enabled) = enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if enabled { 1 } else { 0 })));
            idx += 1;
        }
        if parsed_balance.is_some() {
            set_clauses.push(format!("balance_nano_usd = ${idx}"));
            values.push(new_balance.to_string().into());
            idx += 1;
        }
        if balance_unlimited.is_some() {
            set_clauses.push(format!("balance_unlimited = ${idx}"));
            values.push(SeaValue::Int(Some(if new_unlimited { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(email) = email {
            match email
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(email) => {
                    set_clauses.push(format!("email = ${idx}"));
                    values.push(email.into());
                    idx += 1;
                }
                None => set_clauses.push("email = NULL".to_string()),
            }
        }
        if let Some(allowed_groups_json) = allowed_groups_json {
            set_clauses.push(format!("allowed_groups = ${idx}"));
            values.push(allowed_groups_json.into());
            idx += 1;
        }
        if let Some(plan_assignment) = billing_plan_id {
            match plan_assignment {
                Some(plan_id) => {
                    // Lock the plan row so assignment cannot race delete (BP-D3)
                    // and the anchor matches a surviving plan (BP-S1/BP-S3).
                    let plan_lock_sql = if self.db.is_postgres() {
                        "SELECT period_seconds FROM billing_plans WHERE id = $1 FOR UPDATE"
                    } else {
                        "SELECT period_seconds FROM billing_plans WHERE id = $1"
                    };
                    let plan_row = tx
                        .query_one(self.db.stmt(plan_lock_sql, vec![plan_id.clone().into()]))
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "billing plan not found".to_string())?;
                    let period_seconds: i64 = plan_row
                        .try_get("", "period_seconds")
                        .map_err(|e| e.to_string())?;
                    let anchor_ts = Utc::now()
                        .timestamp()
                        .checked_add(period_seconds)
                        .ok_or_else(|| "next_grant_at overflow".to_string())?;
                    let anchor = DateTime::from_timestamp(anchor_ts, 0)
                        .ok_or_else(|| "next_grant_at overflow".to_string())?;
                    set_clauses.push(format!("billing_plan_id = ${idx}"));
                    values.push(plan_id.into());
                    idx += 1;
                    set_clauses.push(format!("next_grant_at = ${idx}"));
                    values.push(anchor.to_rfc3339().into());
                    idx += 1;
                }
                None => {
                    set_clauses.push("billing_plan_id = NULL".to_string());
                    set_clauses.push("next_grant_at = NULL".to_string());
                }
            }
        }
        set_clauses.push(format!("updated_at = ${idx}"));
        values.push(now.clone().into());
        idx += 1;
        values.push(id.into());
        tx.execute(self.db.stmt(
            &format!(
                "UPDATE users SET {} WHERE id = ${idx}",
                set_clauses.join(", ")
            ),
            values,
        ))
        .await
        .map_err(|e| e.to_string())?;

        if has_balance_change {
            let delta = new_balance
                .checked_sub(current.balance)
                .ok_or_else(|| "balance delta overflow".to_string())?;
            self.insert_billing_ledger_tx(
                &tx,
                id,
                "admin_adjustment",
                delta,
                Some(new_balance),
                &serde_json::json!({
                    "actor_user_id": actor_user_id,
                    "before_balance_nano_usd": current.balance.to_string(),
                    "after_balance_nano_usd": new_balance.to_string(),
                    "before_balance_unlimited": current.unlimited,
                    "after_balance_unlimited": new_unlimited,
                }),
                &now,
            )
            .await
            .map_err(|error| error.message)?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_user_id(id);
        if has_balance_change {
            self.balance_cache.invalidate(id);
        }
        if has_plan_change {
            // Cached auth results embed the plan's group restriction layer.
            self.api_key_cache.invalidate_by_user_id(id);
        }
        Ok(())
    }

    pub async fn delete_user(&self, id: &str) -> Result<(), String> {
        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        let user_lock_sql = if self.db.is_postgres() {
            "SELECT id FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT id FROM users WHERE id = $1"
        };
        let user = tx
            .query_one(self.db.stmt(user_lock_sql, vec![id.into()]))
            .await
            .map_err(|e| e.to_string())?;
        if user.is_none() {
            return Err("user not found".to_string());
        }
        let result = tx
            .execute(
                self.db
                    .stmt("DELETE FROM users WHERE id = $1", vec![id.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;
        if result.rows_affected() != 1 {
            return Err("user not found".to_string());
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_user_id(id);
        self.balance_cache.invalidate(id);
        Ok(())
    }

    pub async fn update_last_login(&self, id: &str) -> Result<(), String> {
        let now = Utc::now();
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "UPDATE users SET last_login_at = $1 WHERE id = $2",
                vec![now.to_rfc3339().into(), id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_user_id(id);
        Ok(())
    }

    pub async fn create_session(
        &self,
        user_id: &str,
        session_ttl_days: i64,
    ) -> Result<Session, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let token = format!(
            "urp_session_{}",
            uuid::Uuid::new_v4().to_string().replace("-", "")
        );
        let now = Utc::now();
        let expires_at = now + chrono::Duration::days(session_ttl_days);

        self.db
            .write()
            .await
            .execute(self.db.stmt(
                r#"INSERT INTO sessions (id, user_id, token, created_at, expires_at)
                   VALUES ($1, $2, $3, $4, $5)"#,
                vec![
                    id.clone().into(),
                    user_id.into(),
                    token.clone().into(),
                    now.to_rfc3339().into(),
                    expires_at.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(Session {
            id,
            user_id: user_id.to_string(),
            token,
            created_at: now,
            expires_at,
        })
    }

    pub async fn cleanup_expired_sessions(&self) -> Result<u64, String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM sessions WHERE expires_at <= $1",
                vec![Utc::now().to_rfc3339().into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    pub async fn get_session_by_token(&self, token: &str) -> Result<Option<Session>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, user_id, token, created_at, expires_at FROM sessions WHERE token = $1",
                vec![token.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            let expires_at: String = row.try_get("", "expires_at").map_err(|e| e.to_string())?;
            let expires_at = DateTime::parse_from_rfc3339(&expires_at)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            if expires_at < Utc::now() {
                self.delete_session(token).await?;
                return Ok(None);
            }

            Ok(Some(Session {
                id: row.try_get("", "id").map_err(|e| e.to_string())?,
                user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                token: row.try_get("", "token").map_err(|e| e.to_string())?,
                created_at: DateTime::parse_from_rfc3339(
                    &row.try_get::<String>("", "created_at")
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc),
                expires_at,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_session(&self, token: &str) -> Result<(), String> {
        self.db
            .write()
            .await
            .execute(
                self.db
                    .stmt("DELETE FROM sessions WHERE token = $1", vec![token.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete_user_sessions(&self, user_id: &str) -> Result<(), String> {
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM sessions WHERE user_id = $1",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

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
                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
                reasoning_envelope_enabled: true,
                request_capture_mode: RequestCaptureMode::Off,
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
        canonicalize_transform_rules(&mut input.transforms);
        validate_api_key_transforms(&input.transforms, is_admin)?;
        validate_model_redirects(&input.model_redirects)?;
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
        let user_allowed_groups = self
            .get_user_by_id(user_id)
            .await?
            .map(|user| user.allowed_groups)
            .unwrap_or_default();
        let allowed_groups = canonicalize_groups(&input.allowed_groups);
        validate_api_key_allowed_groups_subset(&user_allowed_groups, &allowed_groups)?;
        let id = uuid::Uuid::new_v4().to_string();
        let key = format!("sk-{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let key_prefix = key[..12].to_string();
        let key_hash = api_key_lookup_hash(&key);
        let now = Utc::now();
        let expires_at = input
            .expires_in_days
            .map(|days| now + chrono::Duration::days(days));

        let model_limits_json =
            serde_json::to_string(&input.model_limits).map_err(|e| e.to_string())?;
        let ip_whitelist_json =
            serde_json::to_string(&input.ip_whitelist).map_err(|e| e.to_string())?;
        let allowed_groups_json = serialize_allowed_groups_json(&allowed_groups)?;
        let model_redirects_json =
            serde_json::to_string(&input.model_redirects).map_err(|e| e.to_string())?;

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        self.lock_user_balance_tx(&tx, user_id)
            .await
            .map_err(|e| e.message)?;
        tx.execute(self.db.stmt(
                r#"INSERT INTO api_keys (id, user_id, name, key_prefix, key, key_hash, created_at, expires_at, enabled, sub_account_enabled, sub_account_balance_nano, model_limits_enabled, model_limits, ip_whitelist, allowed_groups, token_group, max_multiplier, transforms, model_redirects, reasoning_envelope_enabled, request_capture_enabled, request_capture_mode)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)"#,
                vec![
                    id.clone().into(),
                    user_id.into(),
                    input.name.clone().into(),
                    key_prefix.clone().into(),
                    key.clone().into(),
                    key_hash.clone().into(),
                    now.to_rfc3339().into(),
                    expires_at.map(|e| e.to_rfc3339()).into(),
                    SeaValue::Int(Some(if input.sub_account_enabled { 1 } else { 0 })),
                    initial_sub_account_balance.to_string().into(),
                    SeaValue::Int(Some(if input.model_limits_enabled { 1 } else { 0 })),
                    model_limits_json.into(),
                    ip_whitelist_json.into(),
                    allowed_groups_json.into(),
                    "default".into(),
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
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        if initial_sub_account_balance != 0 {
            self.insert_billing_ledger_tx(
                &tx,
                user_id,
                "admin_sub_account_adjustment",
                initial_sub_account_balance,
                Some(initial_sub_account_balance),
                &serde_json::json!({ "api_key_id": id, "initial": true }),
                &now.to_rfc3339(),
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
            key_hash,
            created_at: now,
            expires_at,
            last_used_at: None,
            enabled: true,
            sub_account_enabled: input.sub_account_enabled,
            sub_account_balance_nano: initial_sub_account_balance.to_string(),
            model_limits_enabled: input.model_limits_enabled,
            model_limits: input.model_limits,
            ip_whitelist: input.ip_whitelist,
            allowed_groups,
            max_multiplier: input.max_multiplier,
            transforms: input.transforms,
            model_redirects: input.model_redirects,
            reasoning_envelope_enabled: input.reasoning_envelope_enabled,
            request_capture_mode: input.request_capture_mode,
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
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.key_hash, a.created_at, a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled, a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits, a.ip_whitelist, a.allowed_groups, a.token_group, a.max_multiplier, a.transforms, a.model_redirects, a.reasoning_envelope_enabled, a.request_capture_enabled, a.request_capture_mode, u.role AS owner_role FROM api_keys a JOIN users u ON u.id = a.user_id WHERE a.key_prefix = $1",
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
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.key_hash, a.created_at, a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled, a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits, a.ip_whitelist, a.allowed_groups, a.token_group, a.max_multiplier, a.transforms, a.model_redirects, a.reasoning_envelope_enabled, a.request_capture_enabled, a.request_capture_mode, u.role AS owner_role FROM api_keys a JOIN users u ON u.id = a.user_id WHERE a.key = $1",
                vec![key.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        match row {
            Some(row) => Ok(Some(self.row_to_api_key(&row).await?)),
            None => Ok(None),
        }
    }

    async fn get_api_key_auth_candidate(
        &self,
        key: &str,
    ) -> Result<Option<(ApiKey, User, Option<Vec<String>>)>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.key_hash,
                        a.created_at, a.expires_at, a.last_used_at, a.enabled,
                        a.sub_account_enabled, a.sub_account_balance_nano,
                        a.model_limits_enabled, a.model_limits, a.ip_whitelist,
                        a.allowed_groups, a.token_group, a.max_multiplier, a.transforms,
                        a.model_redirects, a.reasoning_envelope_enabled,
                        a.request_capture_enabled, a.request_capture_mode,
                        u.role AS owner_role,
                        u.id AS owner_id, u.username AS owner_username,
                        u.password_hash AS owner_password_hash,
                        u.created_at AS owner_created_at, u.updated_at AS owner_updated_at,
                        u.last_login_at AS owner_last_login_at, u.enabled AS owner_enabled,
                        u.balance_nano_usd AS owner_balance_nano_usd,
                        u.balance_unlimited AS owner_balance_unlimited,
                        u.email AS owner_email, u.allowed_groups AS owner_allowed_groups,
                        u.billing_plan_id AS owner_billing_plan_id,
                        u.next_grant_at AS owner_next_grant_at,
                        p.allowed_groups AS plan_allowed_groups
                  FROM api_keys a
                  JOIN users u ON u.id = a.user_id
                  LEFT JOIN billing_plans p ON p.id = u.billing_plan_id AND p.enabled = 1
                  WHERE a.key_hash = $1 AND a.key = $2
                  LIMIT 1",
                vec![api_key_lookup_hash(key).into(), key.into()],
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
        let owner_allowed_groups_raw = row
            .try_get::<Option<String>>("", "owner_allowed_groups")
            .map_err(|error| format!("invalid persisted users.allowed_groups: {error}"))?;
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
            allowed_groups: parse_allowed_groups_json(
                owner_allowed_groups_raw.as_deref(),
                "users.allowed_groups",
            )?,
            billing_plan_id: row
                .try_get::<Option<String>>("", "owner_billing_plan_id")
                .map_err(|e| e.to_string())?,
            next_grant_at: row
                .try_get::<Option<String>>("", "owner_next_grant_at")
                .map_err(|e| e.to_string())?
                .map(|value| DateTime::parse_from_rfc3339(&value).map(|v| v.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| e.to_string())?,
        };
        // A disabled or missing plan contributes no restriction (BP-R2).
        let plan_allowed_groups = row
            .try_get::<Option<String>>("", "plan_allowed_groups")
            .map_err(|e| e.to_string())?
            .map(|raw| {
                parse_allowed_groups_json(Some(raw.as_str()), "billing_plans.allowed_groups")
            })
            .transpose()?;
        Ok(Some((api_key, user, plan_allowed_groups)))
    }

    pub async fn list_user_api_keys(&self, user_id: &str) -> Result<Vec<ApiKey>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.key_hash, a.created_at,
                        a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled,
                        a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits,
                        a.ip_whitelist, a.allowed_groups, a.token_group, a.max_multiplier,
                        a.transforms, a.model_redirects, a.reasoning_envelope_enabled,
                        a.request_capture_enabled, a.request_capture_mode, u.role AS owner_role
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
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.key_hash, a.created_at,
                        a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled,
                        a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits,
                        a.ip_whitelist, a.allowed_groups, a.token_group, a.max_multiplier,
                        a.transforms, a.model_redirects, a.reasoning_envelope_enabled,
                        a.request_capture_enabled, a.request_capture_mode, u.role AS owner_role
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

    async fn lock_user_balance_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
    ) -> Result<LockedUserBalance, BillingError> {
        let sql = if self.db.is_postgres() {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(sql, vec![user_id.into()]))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "user not found"))?;
        let raw: String = row
            .try_get("", "balance_nano_usd")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let balance = parse_nano_usd(&raw)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        let unlimited = row
            .try_get::<i32>("", "balance_unlimited")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            == 1;
        Ok(LockedUserBalance { balance, unlimited })
    }

    async fn lock_api_key_balance_tx(
        &self,
        tx: &DatabaseTransaction,
        api_key_id: &str,
        expected_user_id: &str,
    ) -> Result<LockedApiKeyBalance, BillingError> {
        let sql = if self.db.is_postgres() {
            "SELECT user_id, sub_account_enabled, sub_account_balance_nano FROM api_keys WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT user_id, sub_account_enabled, sub_account_balance_nano FROM api_keys WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(sql, vec![api_key_id.into()]))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "api key not found"))?;
        let user_id: String = row
            .try_get("", "user_id")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        if user_id != expected_user_id {
            return Err(BillingError::new(
                BillingErrorKind::NotFound,
                "api key owner does not match user",
            ));
        }
        let raw: String = row
            .try_get("", "sub_account_balance_nano")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let balance = parse_nano_usd(&raw)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        let sub_account_enabled = row
            .try_get::<i32>("", "sub_account_enabled")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            == 1;
        Ok(LockedApiKeyBalance {
            user_id,
            balance,
            sub_account_enabled,
        })
    }

    async fn delete_api_keys_transactional(&self, ids: &[String]) -> Result<usize, String> {
        if ids.len() > api_key_batch_delete_max_ids() {
            return Err(format!(
                "batch delete accepts at most {} ids",
                api_key_batch_delete_max_ids()
            ));
        }
        if ids.is_empty() {
            return Ok(0);
        }

        let mut key_ids = ids.to_vec();
        key_ids.sort();
        key_ids.dedup();

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;

        let placeholders = (0..key_ids.len())
            .map(|index| format!("${}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let key_values = key_ids.iter().cloned().map(Into::into).collect::<Vec<_>>();
        let owner_rows = tx
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, user_id FROM api_keys WHERE id IN ({placeholders}) ORDER BY user_id, id"
                ),
                key_values.clone(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        let mut expected_owners: BTreeMap<String, String> = BTreeMap::new();
        for row in owner_rows {
            expected_owners.insert(
                row.try_get("", "id").map_err(|e| e.to_string())?,
                row.try_get("", "user_id").map_err(|e| e.to_string())?,
            );
        }

        let user_ids: Vec<String> = expected_owners
            .values()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut user_balances = BTreeMap::new();
        if !user_ids.is_empty() {
            let user_placeholders = (0..user_ids.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let user_lock_suffix = if self.db.is_postgres() {
                " FOR UPDATE"
            } else {
                ""
            };
            let user_rows = tx
                .query_all(self.db.stmt(
                    &format!(
                        "SELECT id, balance_nano_usd, balance_unlimited
                         FROM users WHERE id IN ({user_placeholders})
                         ORDER BY id{user_lock_suffix}"
                    ),
                    user_ids.iter().cloned().map(Into::into).collect(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            for row in user_rows {
                let user_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
                let raw: String = row
                    .try_get("", "balance_nano_usd")
                    .map_err(|e| e.to_string())?;
                let balance = parse_nano_usd(&raw)
                    .map_err(|e| format!("invalid persisted user balance: {e}"))?;
                let unlimited = row
                    .try_get::<i32>("", "balance_unlimited")
                    .map_err(|e| e.to_string())?
                    == 1;
                user_balances.insert(user_id, LockedUserBalance { balance, unlimited });
            }
            if user_balances.len() != user_ids.len() {
                return Err("api key owner was not found".to_string());
            }
        }

        let lock_suffix = if self.db.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        };
        let locked_rows = tx
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, user_id, sub_account_enabled, sub_account_balance_nano
                     FROM api_keys WHERE id IN ({placeholders})
                     ORDER BY user_id, id{lock_suffix}"
                ),
                key_values.clone(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        let mut locked_keys = Vec::with_capacity(locked_rows.len());
        for row in locked_rows {
            let key_id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let user_id: String = row.try_get("", "user_id").map_err(|e| e.to_string())?;
            if expected_owners.get(&key_id) != Some(&user_id) {
                continue;
            }
            let raw_balance: String = row
                .try_get("", "sub_account_balance_nano")
                .map_err(|e| e.to_string())?;
            let balance = parse_nano_usd(&raw_balance)
                .map_err(|e| format!("invalid persisted sub-account balance: {e}"))?;
            locked_keys.push((
                key_id,
                LockedApiKeyBalance {
                    user_id,
                    balance,
                    sub_account_enabled: row
                        .try_get::<i32>("", "sub_account_enabled")
                        .map_err(|e| e.to_string())?
                        == 1,
                },
            ));
        }

        let now = Utc::now().to_rfc3339();
        let deleted_key_ids = locked_keys
            .iter()
            .map(|(key_id, _)| key_id.clone())
            .collect::<Vec<_>>();
        let mut affected_user_ids = BTreeSet::new();
        let mut user_updates = BTreeMap::new();
        let mut settlement_rows = Vec::new();
        for (key_id, key) in &locked_keys {
            if key.balance != 0 {
                let user = user_balances
                    .get_mut(&key.user_id)
                    .ok_or_else(|| "locked user balance missing".to_string())?;
                let balance_after = if user.unlimited {
                    None
                } else {
                    let next = user
                        .balance
                        .checked_add(key.balance)
                        .ok_or_else(|| "sub-account delete settlement overflow".to_string())?;
                    user.balance = next;
                    user_updates.insert(key.user_id.clone(), next);
                    Some(next)
                };
                settlement_rows.push((
                    uuid::Uuid::new_v4().to_string(),
                    key.user_id.clone(),
                    key.balance,
                    balance_after,
                    serde_json::json!({ "api_key_id": key_id }).to_string(),
                ));
                affected_user_ids.insert(key.user_id.clone());
            }
        }

        const USER_UPDATE_CHUNK_SIZE: usize = 199;
        let user_updates = user_updates.into_iter().collect::<Vec<_>>();
        for chunk in user_updates.chunks(USER_UPDATE_CHUNK_SIZE) {
            let mut values = Vec::with_capacity(chunk.len() * 2 + 1);
            let mut cases = Vec::with_capacity(chunk.len());
            let mut ids = Vec::with_capacity(chunk.len());
            for (user_id, balance) in chunk {
                let id_index = values.len() + 1;
                values.push(user_id.clone().into());
                ids.push(format!("${id_index}"));
                let balance_index = values.len() + 1;
                values.push(balance.to_string().into());
                cases.push(format!("WHEN ${id_index} THEN ${balance_index}"));
            }
            let updated_at_index = values.len() + 1;
            values.push(now.clone().into());
            tx.execute(self.db.stmt(
                &format!(
                    "UPDATE users
                     SET balance_nano_usd = CASE id {} ELSE balance_nano_usd END,
                         updated_at = ${updated_at_index}
                     WHERE id IN ({})",
                    cases.join(" "),
                    ids.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        const LEDGER_INSERT_CHUNK_SIZE: usize = 57;
        for chunk in settlement_rows.chunks(LEDGER_INSERT_CHUNK_SIZE) {
            let mut values = Vec::with_capacity(chunk.len() * 7);
            let mut rows = Vec::with_capacity(chunk.len());
            for (id, user_id, delta, balance_after, meta_json) in chunk {
                let start = values.len() + 1;
                values.push(id.clone().into());
                values.push(user_id.clone().into());
                values.push("sub_account_delete_settlement".into());
                values.push(delta.to_string().into());
                values.push(balance_after.map(|value| value.to_string()).into());
                values.push(meta_json.clone().into());
                values.push(now.clone().into());
                rows.push(format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    start,
                    start + 1,
                    start + 2,
                    start + 3,
                    start + 4,
                    start + 5,
                    start + 6
                ));
            }
            tx.execute(self.db.stmt(
                &format!(
                    "INSERT INTO billing_ledger
                     (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at)
                     VALUES {}",
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }
        if !deleted_key_ids.is_empty() {
            let delete_placeholders = (0..deleted_key_ids.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            tx.execute(self.db.stmt(
                &format!("DELETE FROM api_keys WHERE id IN ({delete_placeholders})"),
                deleted_key_ids.iter().cloned().map(Into::into).collect(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        self.api_key_cache.invalidate_by_key_ids(&key_ids);
        for user_id in affected_user_ids {
            self.balance_cache.invalidate(&user_id);
        }
        Ok(deleted_key_ids.len())
    }

    pub async fn delete_api_key(&self, id: &str) -> Result<(), String> {
        self.delete_api_keys_transactional(&[id.to_string()])
            .await?;
        Ok(())
    }

    pub async fn validate_api_key(
        &self,
        key: &str,
    ) -> Result<Option<(ApiKey, User, Option<Vec<String>>)>, String> {
        if key.len() < 12 || key.len() > MAX_FORWARDING_API_KEY_BYTES {
            return Ok(None);
        }

        loop {
            if let Some((cached_key, cached_user, cached_plan_groups)) = self.api_key_cache.get(key)
            {
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
                    return Ok(Some((cached_key, cached_user, cached_plan_groups)));
                }

                self.api_key_cache.invalidate(key);
            }

            let generation = self.api_key_cache.current_generation();
            let (api_key, user, plan_allowed_groups) =
                match self.get_api_key_auth_candidate(key).await? {
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
                plan_allowed_groups.clone(),
            ) {
                continue;
            }

            self.last_used_batcher
                .record(api_key.id.clone(), Utc::now());

            return Ok(Some((api_key, user, plan_allowed_groups)));
        }
    }

    pub(crate) fn row_to_user(&self, row: &QueryResult) -> Result<User, String> {
        let role_str: String = row.try_get("", "role").map_err(|e| e.to_string())?;
        let role = UserRole::from_str(&role_str).ok_or_else(|| "invalid role".to_string())?;

        let last_login_at: Option<String> = row
            .try_get("", "last_login_at")
            .map_err(|e| e.to_string())?;
        let last_login_at = last_login_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;
        let allowed_groups_raw = row
            .try_get::<Option<String>>("", "allowed_groups")
            .map_err(|error| format!("invalid persisted users.allowed_groups: {error}"))?;
        let allowed_groups =
            parse_allowed_groups_json(allowed_groups_raw.as_deref(), "users.allowed_groups")?;
        let billing_plan_id: Option<String> = row
            .try_get("", "billing_plan_id")
            .map_err(|e| e.to_string())?;
        let next_grant_at: Option<String> = row
            .try_get("", "next_grant_at")
            .map_err(|e| e.to_string())?;
        let next_grant_at = next_grant_at
            .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
            .transpose()
            .map_err(|e| e.to_string())?;
        if billing_plan_id.is_some() != next_grant_at.is_some() {
            return Err(
                "invalid persisted user: billing_plan_id and next_grant_at must be set together"
                    .to_string(),
            );
        }
        let balance_nano_usd: String = row
            .try_get("", "balance_nano_usd")
            .map_err(|e| e.to_string())?;
        parse_nano_usd(&balance_nano_usd)
            .map_err(|e| format!("invalid persisted user balance: {e}"))?;

        Ok(User {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            username: row.try_get("", "username").map_err(|e| e.to_string())?,
            password_hash: row
                .try_get("", "password_hash")
                .map_err(|e| e.to_string())?,
            role,
            created_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", "created_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(
                &row.try_get::<String>("", "updated_at")
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc),
            last_login_at,
            enabled: decode_required_bool(row, "enabled")?,
            balance_nano_usd,
            balance_unlimited: row
                .try_get::<i32>("", "balance_unlimited")
                .map_err(|e| e.to_string())?
                == 1,
            email: row
                .try_get::<Option<String>>("", "email")
                .map_err(|e| e.to_string())?,
            allowed_groups,
            billing_plan_id,
            next_grant_at,
        })
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
        let allowed_groups_raw = row
            .try_get::<Option<String>>("", "allowed_groups")
            .map_err(|error| format!("invalid persisted api_keys.allowed_groups: {error}"))?;
        let allowed_groups =
            parse_allowed_groups_json(allowed_groups_raw.as_deref(), "api_keys.allowed_groups")?;

        let max_multiplier = row
            .try_get::<Option<String>>("", "max_multiplier")
            .map_err(|e| e.to_string())?
            .map(|value| value.parse())
            .transpose()
            .map_err(|e: String| format!("invalid persisted max_multiplier: {e}"))?;
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
            sanitize_api_key_transforms(transforms, is_admin);
        let model_redirects: Vec<ModelRedirectRule> =
            parse_persisted_json_array(&model_redirects_str, "model_redirects")?;
        validate_model_redirects(&model_redirects)
            .map_err(|error| format!("invalid persisted model_redirects: {error}"))?;
        let reasoning_envelope_enabled = decode_required_bool(row, "reasoning_envelope_enabled")?;
        let request_capture_mode = decode_request_capture_mode(row)?;

        Ok(ApiKey {
            id: row.try_get("", "id").map_err(|e| e.to_string())?,
            user_id,
            name: row.try_get("", "name").map_err(|e| e.to_string())?,
            key_prefix: row.try_get("", "key_prefix").map_err(|e| e.to_string())?,
            key: row.try_get("", "key").map_err(|e| e.to_string())?,
            key_hash: row.try_get("", "key_hash").map_err(|e| e.to_string())?,
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
            allowed_groups,
            max_multiplier,
            transforms,
            model_redirects,
            reasoning_envelope_enabled,
            request_capture_mode,
        })
    }

    /// Update an existing API key with new fields
    pub async fn update_api_key(
        &self,
        key_id: &str,
        input: UpdateApiKeyInput,
        is_admin: bool,
    ) -> Result<ApiKey, String> {
        if let Some(transforms) = &input.transforms {
            validate_api_key_transforms(transforms, is_admin)?;
        }
        if let Some(model_redirects) = &input.model_redirects {
            validate_model_redirects(model_redirects)?;
        }
        let canonical_ip_whitelist = input
            .ip_whitelist
            .as_ref()
            .map(|entries| canonicalize_ip_whitelist(entries))
            .transpose()?;
        if input.sub_account_balance_nano_usd.is_some() && !is_admin {
            return Err("only admins may set a sub-account balance".to_string());
        }
        if input.sub_account_enabled == Some(false) && input.sub_account_balance_nano_usd.is_some()
        {
            return Err(
                "sub-account balance cannot be supplied while disabling sub-account billing"
                    .to_string(),
            );
        }
        let requested_sub_account_balance = input
            .sub_account_balance_nano_usd
            .as_deref()
            .map(parse_nano_usd)
            .transpose()?;
        let existing_key = self
            .get_api_key_by_id(key_id)
            .await?
            .ok_or_else(|| "API key not found".to_string())?;
        let resulting_sub_account_enabled = input
            .sub_account_enabled
            .unwrap_or(existing_key.sub_account_enabled);
        if requested_sub_account_balance.is_some_and(|balance| balance != 0)
            && !resulting_sub_account_enabled
        {
            return Err(
                "a non-zero sub-account balance requires sub-account billing to be enabled"
                    .to_string(),
            );
        }
        let disabling_sub_account = input.sub_account_enabled == Some(false);
        let allowed_groups = input
            .allowed_groups
            .as_ref()
            .map(|groups| canonicalize_groups(groups))
            .unwrap_or_else(|| existing_key.allowed_groups.clone());
        let mut set_clauses = Vec::new();
        let mut values: Vec<SeaValue> = Vec::new();
        let mut idx = 1usize;

        if let Some(name) = &input.name {
            set_clauses.push(format!("name = ${idx}"));
            values.push(name.clone().into());
            idx += 1;
        }
        if let Some(enabled) = input.enabled {
            set_clauses.push(format!("enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if enabled { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(sub_account_enabled) = input.sub_account_enabled {
            set_clauses.push(format!("sub_account_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if sub_account_enabled { 1 } else { 0 })));
            idx += 1;
        }
        if let Some(sub_account_balance) = requested_sub_account_balance {
            set_clauses.push(format!("sub_account_balance_nano = ${idx}"));
            values.push(sub_account_balance.to_string().into());
            idx += 1;
        }
        if let Some(model_limits_enabled) = input.model_limits_enabled {
            set_clauses.push(format!("model_limits_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if model_limits_enabled {
                1
            } else {
                0
            })));
            idx += 1;
        }
        if let Some(model_limits) = &input.model_limits {
            set_clauses.push(format!("model_limits = ${idx}"));
            values.push(
                serde_json::to_string(model_limits)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if let Some(ip_whitelist) = &canonical_ip_whitelist {
            set_clauses.push(format!("ip_whitelist = ${idx}"));
            values.push(
                serde_json::to_string(ip_whitelist)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if input.allowed_groups.is_some() {
            set_clauses.push(format!("allowed_groups = ${idx}"));
            values.push(serialize_allowed_groups_json(&allowed_groups)?.into());
            idx += 1;
        }
        if let Some(max_multiplier) = input.max_multiplier {
            set_clauses.push(format!("max_multiplier = ${idx}"));
            values.push(max_multiplier.to_string().into());
            idx += 1;
        }
        if let Some(transforms) = &input.transforms {
            let mut transforms = transforms.clone();
            canonicalize_transform_rules(&mut transforms);
            set_clauses.push(format!("transforms = ${idx}"));
            values.push(
                serde_json::to_string(&transforms)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if let Some(model_redirects) = &input.model_redirects {
            set_clauses.push(format!("model_redirects = ${idx}"));
            values.push(
                serde_json::to_string(model_redirects)
                    .map_err(|e| e.to_string())?
                    .into(),
            );
            idx += 1;
        }
        if let Some(reasoning_envelope_enabled) = input.reasoning_envelope_enabled {
            set_clauses.push(format!("reasoning_envelope_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(if reasoning_envelope_enabled {
                1
            } else {
                0
            })));
            idx += 1;
        }
        if let Some(request_capture_mode) = input.request_capture_mode {
            set_clauses.push(format!("request_capture_enabled = ${idx}"));
            values.push(SeaValue::Int(Some(
                if request_capture_mode.should_start_capture() {
                    1
                } else {
                    0
                },
            )));
            idx += 1;
            set_clauses.push(format!("request_capture_mode = ${idx}"));
            values.push(request_capture_mode.as_str().into());
            idx += 1;
        }
        if let Some(expires_at) = &input.expires_at {
            set_clauses.push(format!("expires_at = ${idx}"));
            values.push(expires_at.clone().into());
            idx += 1;
        }

        if set_clauses.is_empty() {
            return Ok(existing_key);
        }

        let user_allowed_groups = self
            .get_user_by_id(&existing_key.user_id)
            .await?
            .map(|user| user.allowed_groups)
            .unwrap_or_default();
        validate_api_key_allowed_groups_subset(&user_allowed_groups, &allowed_groups)?;

        values.push(key_id.into());

        let query = format!(
            "UPDATE api_keys SET {} WHERE id = ${idx}",
            set_clauses.join(", ")
        );

        if disabling_sub_account || requested_sub_account_balance.is_some() {
            let write = self.db.write().await;
            let tx = write.begin().await.map_err(|e| e.to_string())?;
            let mut user = self
                .lock_user_balance_tx(&tx, &existing_key.user_id)
                .await
                .map_err(|e| e.message)?;
            let key = self
                .lock_api_key_balance_tx(&tx, key_id, &existing_key.user_id)
                .await
                .map_err(|e| e.message)?;

            if disabling_sub_account && (key.sub_account_enabled || key.balance != 0) {
                let balance_after = if user.unlimited {
                    None
                } else {
                    let next = user
                        .balance
                        .checked_add(key.balance)
                        .ok_or_else(|| "sub-account disable settlement overflow".to_string())?;
                    tx.execute(self.db.stmt(
                        "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                        vec![
                            next.to_string().into(),
                            Utc::now().to_rfc3339().into(),
                            existing_key.user_id.clone().into(),
                        ],
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                    user.balance = next;
                    Some(next)
                };

                tx.execute(self.db.stmt(
                    "UPDATE api_keys SET sub_account_balance_nano = '0' WHERE id = $1",
                    vec![key_id.into()],
                ))
                .await
                .map_err(|e| e.to_string())?;

                if key.balance != 0 {
                    let now = Utc::now().to_rfc3339();
                    let kind = if key.balance > 0 {
                        "sub_account_refund"
                    } else {
                        "sub_account_debt_transfer"
                    };
                    self.insert_billing_ledger_tx(
                        &tx,
                        &existing_key.user_id,
                        kind,
                        key.balance,
                        balance_after,
                        &serde_json::json!({ "api_key_id": key_id }),
                        &now,
                    )
                    .await
                    .map_err(|e| e.message)?;
                }
            } else if let Some(new_balance) = requested_sub_account_balance {
                let now = Utc::now().to_rfc3339();
                if new_balance < key.balance {
                    let refund = key
                        .balance
                        .checked_sub(new_balance)
                        .ok_or_else(|| "sub-account refund overflow".to_string())?;
                    let balance_after = if user.unlimited {
                        None
                    } else {
                        let next = user
                            .balance
                            .checked_add(refund)
                            .ok_or_else(|| "sub-account refund overflow".to_string())?;
                        tx.execute(self.db.stmt(
                            "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                            vec![
                                next.to_string().into(),
                                now.clone().into(),
                                existing_key.user_id.clone().into(),
                            ],
                        ))
                        .await
                        .map_err(|e| e.to_string())?;
                        user.balance = next;
                        Some(next)
                    };
                    self.insert_billing_ledger_tx(
                        &tx,
                        &existing_key.user_id,
                        "sub_account_refund",
                        refund,
                        balance_after,
                        &serde_json::json!({
                            "api_key_id": key_id,
                            "balance_before_nano_usd": key.balance.to_string(),
                            "balance_after_nano_usd": new_balance.to_string(),
                        }),
                        &now,
                    )
                    .await
                    .map_err(|e| e.message)?;
                } else if new_balance > key.balance {
                    let increase = new_balance
                        .checked_sub(key.balance)
                        .ok_or_else(|| "sub-account adjustment overflow".to_string())?;
                    self.insert_billing_ledger_tx(
                        &tx,
                        &existing_key.user_id,
                        "admin_sub_account_adjustment",
                        increase,
                        Some(new_balance),
                        &serde_json::json!({
                            "api_key_id": key_id,
                            "balance_before_nano_usd": key.balance.to_string(),
                            "balance_after_nano_usd": new_balance.to_string(),
                        }),
                        &now,
                    )
                    .await
                    .map_err(|e| e.message)?;
                }
            }

            tx.execute(self.db.stmt(&query, values))
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
            if disabling_sub_account
                || requested_sub_account_balance.is_some_and(|balance| balance < key.balance)
            {
                self.balance_cache.invalidate(&existing_key.user_id);
            }
        } else {
            self.db
                .write()
                .await
                .execute(self.db.stmt(&query, values))
                .await
                .map_err(|e| e.to_string())?;
        }

        self.api_key_cache.invalidate_by_key_id(key_id);

        self.get_api_key_by_id(key_id)
            .await?
            .ok_or_else(|| "API key not found after update".to_string())
    }

    /// Get API key by ID
    pub async fn get_api_key_by_id(&self, id: &str) -> Result<Option<ApiKey>, String> {
        let row = self.db.read()
            .query_one(self.db.stmt(
                "SELECT a.id, a.user_id, a.name, a.key_prefix, a.key, a.key_hash, a.created_at, a.expires_at, a.last_used_at, a.enabled, a.sub_account_enabled, a.sub_account_balance_nano, a.model_limits_enabled, a.model_limits, a.ip_whitelist, a.allowed_groups, a.token_group, a.max_multiplier, a.transforms, a.model_redirects, a.reasoning_envelope_enabled, a.request_capture_enabled, a.request_capture_mode, u.role AS owner_role FROM api_keys a JOIN users u ON u.id = a.user_id WHERE a.id = $1",
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

    pub async fn get_user_balance(&self, user_id: &str) -> Result<Option<UserBalance>, String> {
        loop {
            if let Some(cached) = self.balance_cache.get(user_id) {
                return Ok(Some(cached));
            }
            let generation = self.balance_cache.current_generation();
            let row = self
                .db
                .read()
                .query_one(self.db.stmt(
                    "SELECT id, balance_nano_usd, balance_unlimited FROM users WHERE id = $1",
                    vec![user_id.into()],
                ))
                .await
                .map_err(|e| e.to_string())?;
            let Some(row) = row else {
                if self.balance_cache.current_generation() != generation {
                    continue;
                }
                return Ok(None);
            };
            let balance_raw: String = row
                .try_get("", "balance_nano_usd")
                .map_err(|e| e.to_string())?;
            let balance_nano_usd = parse_nano_usd(&balance_raw)?;
            let balance = UserBalance {
                user_id: row.try_get("", "id").map_err(|e| e.to_string())?,
                balance_nano_usd,
                balance_unlimited: row
                    .try_get::<i32>("", "balance_unlimited")
                    .map_err(|e| e.to_string())?
                    == 1,
            };
            if !self.balance_cache.insert_if_current(
                user_id.to_string(),
                generation,
                balance.clone(),
            ) {
                continue;
            }
            return Ok(Some(balance));
        }
    }

    pub async fn ensure_user_can_spend(&self, user_id: &str) -> Result<(), BillingError> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT balance_nano_usd, balance_unlimited FROM users WHERE id = $1",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "user not found"))?;
        let raw: String = row
            .try_get("", "balance_nano_usd")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let balance = parse_nano_usd(&raw)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        let unlimited = row
            .try_get::<i32>("", "balance_unlimited")
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?
            == 1;

        if unlimited {
            return Ok(());
        }
        if balance <= 0 {
            return Err(BillingError::new(
                BillingErrorKind::InsufficientBalance,
                "insufficient balance",
            ));
        }
        Ok(())
    }

    pub async fn charge_user_balance_nano(
        &self,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<(), BillingError> {
        if amount_nano_usd <= 0 {
            return Ok(());
        }
        if meta
            .get("request_id")
            .and_then(Value::as_str)
            .is_none_or(|request_id| request_id.trim().is_empty())
        {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "request charge metadata is missing request_id",
            ));
        }
        self.charge_user_balance_nano_inner(user_id, amount_nano_usd, meta)
            .await
    }

    async fn charge_user_balance_nano_inner(
        &self,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<(), BillingError> {
        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        let user = self.lock_user_balance_tx(&tx, user_id).await?;
        if user.unlimited {
            tx.commit()
                .await
                .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            return Ok(());
        }

        let next_balance = user.balance.checked_sub(amount_nano_usd).ok_or_else(|| {
            BillingError::new(BillingErrorKind::Overflow, "balance subtraction overflow")
        })?;

        let now = Utc::now().to_rfc3339();
        tx.execute(self.db.stmt(
            "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
            vec![
                next_balance.to_string().into(),
                now.clone().into(),
                user_id.into(),
            ],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "request_charge",
            -amount_nano_usd,
            Some(next_balance),
            meta,
            &now,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        self.balance_cache.invalidate(user_id);
        Ok(())
    }

    pub async fn admin_adjust_user_balance(
        &self,
        user_id: &str,
        balance_nano_usd: Option<String>,
        balance_unlimited: Option<bool>,
        actor_user_id: &str,
    ) -> Result<(), String> {
        if balance_nano_usd.is_none() && balance_unlimited.is_none() {
            return Ok(());
        }

        let _write_guard = self.db.write().await;
        let tx = _write_guard.begin().await.map_err(|e| e.to_string())?;
        let current = self
            .lock_user_balance_tx(&tx, user_id)
            .await
            .map_err(|e| e.message)?;
        let current_balance = current.balance;
        let current_unlimited = current.unlimited;

        let new_balance = if let Some(balance_raw) = balance_nano_usd {
            parse_nano_usd(&balance_raw)?
        } else {
            current_balance
        };
        let new_unlimited = balance_unlimited.unwrap_or(current_unlimited);

        let now = Utc::now().to_rfc3339();
        tx.execute(self.db.stmt(
            "UPDATE users SET balance_nano_usd = $1, balance_unlimited = $2, updated_at = $3 WHERE id = $4",
            vec![
                new_balance.to_string().into(),
                SeaValue::Int(Some(if new_unlimited { 1 } else { 0 })),
                now.clone().into(),
                user_id.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;

        let delta = new_balance
            .checked_sub(current_balance)
            .ok_or_else(|| "balance delta overflow".to_string())?;
        let meta = serde_json::json!({
            "actor_user_id": actor_user_id,
            "before_balance_nano_usd": current_balance.to_string(),
            "after_balance_nano_usd": new_balance.to_string(),
            "before_balance_unlimited": current_unlimited,
            "after_balance_unlimited": new_unlimited,
        });

        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "admin_adjustment",
            delta,
            Some(new_balance),
            &meta,
            &now,
        )
        .await
        .map_err(|e| e.message)?;

        tx.commit().await.map_err(|e| e.to_string())?;
        self.balance_cache.invalidate(user_id);
        Ok(())
    }

    pub async fn charge_sub_account_balance_nano(
        &self,
        api_key_id: &str,
        user_id: &str,
        amount_nano_usd: i128,
        meta: &Value,
    ) -> Result<(), BillingError> {
        if amount_nano_usd <= 0 {
            return Ok(());
        }
        if meta
            .get("request_id")
            .and_then(Value::as_str)
            .is_none_or(|request_id| request_id.trim().is_empty())
        {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "request charge metadata is missing request_id",
            ));
        }
        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        let user = self.lock_user_balance_tx(&tx, user_id).await?;
        let key = match self.lock_api_key_balance_tx(&tx, api_key_id, user_id).await {
            Ok(key) => Some(key),
            Err(error) if error.kind == BillingErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if key.as_ref().is_none_or(|key| !key.sub_account_enabled) {
            if user.unlimited {
                tx.commit()
                    .await
                    .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
                return Ok(());
            }
            let next_balance = user.balance.checked_sub(amount_nano_usd).ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "balance subtraction overflow")
            })?;
            let now = Utc::now().to_rfc3339();
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1, updated_at = $2 WHERE id = $3",
                vec![
                    next_balance.to_string().into(),
                    now.clone().into(),
                    user_id.into(),
                ],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            self.insert_billing_ledger_tx(
                &tx,
                user_id,
                "request_charge",
                -amount_nano_usd,
                Some(next_balance),
                meta,
                &now,
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            self.balance_cache.invalidate(user_id);
            self.api_key_cache.invalidate_by_key_id(api_key_id);
            return Ok(());
        }
        let key = key.expect("enabled sub-account key must be present");
        let next_balance = key.balance.checked_sub(amount_nano_usd).ok_or_else(|| {
            BillingError::new(
                BillingErrorKind::Overflow,
                "sub-account balance subtraction overflow",
            )
        })?;

        let now = Utc::now().to_rfc3339();
        tx.execute(self.db.stmt(
            "UPDATE api_keys SET sub_account_balance_nano = $1 WHERE id = $2",
            vec![next_balance.to_string().into(), api_key_id.into()],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "api_key_charge",
            -amount_nano_usd,
            Some(next_balance),
            meta,
            &now,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        self.api_key_cache.invalidate_by_key_id(api_key_id);
        Ok(())
    }

    pub async fn transfer_to_sub_account(
        &self,
        api_key_id: &str,
        user_id: &str,
        amount_nano_usd: i128,
    ) -> Result<(i128, i128), BillingError> {
        if amount_nano_usd <= 0 {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "transfer amount must be positive",
            ));
        }

        let _write_guard = self.db.write().await;
        let tx = _write_guard
            .begin()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        let user = self.lock_user_balance_tx(&tx, user_id).await?;
        let key = self
            .lock_api_key_balance_tx(&tx, api_key_id, user_id)
            .await?;
        if !key.sub_account_enabled {
            return Err(BillingError::new(
                BillingErrorKind::Internal,
                "sub-account not enabled on this key",
            ));
        }

        let new_user_balance = if user.unlimited {
            user.balance
        } else {
            let next = user.balance.checked_sub(amount_nano_usd).ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "user balance overflow")
            })?;
            if next < 0 {
                return Err(BillingError::new(
                    BillingErrorKind::InsufficientBalance,
                    "insufficient balance for transfer",
                ));
            }
            tx.execute(self.db.stmt(
                "UPDATE users SET balance_nano_usd = $1 WHERE id = $2",
                vec![next.to_string().into(), user_id.into()],
            ))
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
            next
        };

        let new_key_balance = key.balance.checked_add(amount_nano_usd).ok_or_else(|| {
            BillingError::new(BillingErrorKind::Overflow, "sub-account balance overflow")
        })?;

        tx.execute(self.db.stmt(
            "UPDATE api_keys SET sub_account_balance_nano = $1 WHERE id = $2",
            vec![new_key_balance.to_string().into(), api_key_id.into()],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;

        let now = Utc::now().to_rfc3339();
        if !user.unlimited {
            self.insert_billing_ledger_tx(
                &tx,
                user_id,
                "sub_account_transfer_out",
                -amount_nano_usd,
                Some(new_user_balance),
                &serde_json::json!({ "api_key_id": api_key_id }),
                &now,
            )
            .await?;
        }
        self.insert_billing_ledger_tx(
            &tx,
            user_id,
            "sub_account_transfer_in",
            amount_nano_usd,
            Some(new_key_balance),
            &serde_json::json!({ "api_key_id": api_key_id }),
            &now,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        self.balance_cache.invalidate(user_id);
        self.api_key_cache.invalidate_by_key_id(api_key_id);
        Ok((new_key_balance, new_user_balance))
    }

    pub async fn ensure_sub_account_can_spend(&self, api_key_id: &str) -> Result<(), BillingError> {
        let key = self
            .get_api_key_by_id(api_key_id)
            .await
            .map_err(|e| BillingError::new(BillingErrorKind::Internal, e))?
            .ok_or_else(|| BillingError::new(BillingErrorKind::NotFound, "api key not found"))?;
        let balance = parse_nano_usd(&key.sub_account_balance_nano)
            .map_err(|e| BillingError::new(BillingErrorKind::InvalidStoredBalance, e))?;
        if balance <= 0 {
            return Err(BillingError::new(
                BillingErrorKind::InsufficientBalance,
                "insufficient balance",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_billing_ledger_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
        kind: &str,
        delta_nano_usd: i128,
        balance_after_nano_usd: Option<i128>,
        meta: &Value,
        created_at_rfc3339: &str,
    ) -> Result<(), BillingError> {
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(self.db.stmt(
            r#"INSERT INTO billing_ledger (id, user_id, kind, delta_nano_usd, balance_after_nano_usd, meta_json, created_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            vec![
                id.into(),
                user_id.into(),
                kind.into(),
                delta_nano_usd.to_string().into(),
                balance_after_nano_usd.map(|v| v.to_string()).into(),
                meta.to_string().into(),
                created_at_rfc3339.into(),
            ],
        ))
        .await
        .map_err(|e| BillingError::new(BillingErrorKind::Internal, e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SESSION_CLEANUP_INTERVAL_SECS, api_key_lookup_hash, canonicalize_ip_whitelist,
        parse_allowed_groups_json, parse_api_key_batch_delete_limit, parse_positive_limit,
        parse_session_cleanup_interval_secs, sanitize_api_key_transforms,
        serialize_allowed_groups_json, validate_api_key_allowed_groups_subset,
        validate_api_key_transforms,
    };
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::transforms::{Phase, TransformRuleConfig};
    use crate::users::{
        AdminUpdateUserInput, CreateApiKeyInput, CreateApiKeyWithLimitError, RegisterUserError,
        RequestCaptureMode, UserRole, UserStore,
    };
    use chrono::Utc;
    use sea_orm::{ConnectionTrait, Value as SeaValue};
    use sea_orm_migration::MigratorTrait;
    use serde_json::json;

    #[test]
    fn api_key_lookup_hash_uses_complete_token() {
        assert_ne!(
            api_key_lookup_hash("sk-123456789-suffix-a"),
            api_key_lookup_hash("sk-123456789-suffix-b")
        );
        assert_eq!(
            api_key_lookup_hash("sk-stable"),
            api_key_lookup_hash("sk-stable")
        );
    }

    #[test]
    fn api_key_batch_limit_parser_rejects_non_positive_values() {
        assert_eq!(parse_positive_limit(Some("399"), 400), 399);
        assert_eq!(parse_positive_limit(Some("0"), 400), 400);
        assert_eq!(parse_positive_limit(Some("-1"), 400), 400);
        assert_eq!(parse_positive_limit(Some("invalid"), 400), 400);
        assert_eq!(parse_api_key_batch_delete_limit(Some("401")), 400);
    }

    #[test]
    fn session_cleanup_interval_parser_requires_positive_whole_seconds() {
        assert_eq!(parse_session_cleanup_interval_secs(Some("17")), 17);
        for invalid in [
            None,
            Some(""),
            Some("0"),
            Some("-1"),
            Some("invalid"),
            Some("18446744073709551616"),
        ] {
            assert_eq!(
                parse_session_cleanup_interval_secs(invalid),
                DEFAULT_SESSION_CLEANUP_INTERVAL_SECS
            );
        }
    }

    #[tokio::test]
    async fn compatibility_migrations_cross_the_fixed_batch_boundary() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db.clone(), log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("hash-migration", "password", UserRole::User, &[])
            .await
            .expect("user creates");
        for index in 0..305 {
            store
                .create_api_key(&user.id, &format!("key-{index}"), None)
                .await
                .expect("key creates");
        }
        db.write()
            .await
            .execute(db.stmt("UPDATE api_keys SET key_hash = ''", vec![]))
            .await
            .expect("hashes clear");

        store
            .migrate_api_key_lookup_hashes()
            .await
            .expect("hashes migrate");
        let rows = db
            .read()
            .query_all(db.stmt("SELECT key, key_hash FROM api_keys", vec![]))
            .await
            .expect("hashes query");
        assert_eq!(rows.len(), 305);
        for row in rows {
            let key: String = row.try_get("", "key").expect("key decodes");
            let hash: String = row.try_get("", "key_hash").expect("hash decodes");
            assert_eq!(hash, api_key_lookup_hash(&key));
        }

        let legacy = serde_json::to_string(&vec![TransformRuleConfig {
            transform: "remove_anthropic_billing_header".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Request,
            config: json!({}),
        }])
        .unwrap();
        db.write()
            .await
            .execute(db.stmt("UPDATE api_keys SET transforms = $1", vec![legacy.into()]))
            .await
            .expect("legacy transforms seed");
        db.write()
            .await
            .execute(db.stmt(
                "DELETE FROM system_settings WHERE key = $1",
                vec!["migration.api_key_transform_rule_ids.v1".into()],
            ))
            .await
            .expect("migration marker clears");

        store
            .migrate_transform_rule_ids()
            .await
            .expect("transforms migrate");
        let rows = db
            .read()
            .query_all(db.stmt("SELECT transforms FROM api_keys", vec![]))
            .await
            .expect("transforms query");
        assert_eq!(rows.len(), 305);
        for row in rows {
            let raw: String = row.try_get("", "transforms").expect("transforms decode");
            let transforms: Vec<TransformRuleConfig> =
                serde_json::from_str(&raw).expect("transforms parse");
            assert_eq!(transforms[0].transform, "strip_anthropic_billing_header");
        }
    }

    #[tokio::test]
    async fn update_last_login_invalidates_cached_user_for_api_keys() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db, log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("last-login-cache", "password", UserRole::User, &[])
            .await
            .expect("user creates");
        let (_, token) = store
            .create_api_key(&user.id, "cached-key", None)
            .await
            .expect("key creates");

        let (_, cached_user, _) = store
            .validate_api_key(&token)
            .await
            .expect("initial validation succeeds")
            .expect("key is valid");
        assert!(cached_user.last_login_at.is_none());
        assert!(store.api_key_cache.get(&token).is_some());

        store
            .update_last_login(&user.id)
            .await
            .expect("last login updates");
        assert!(store.api_key_cache.get(&token).is_none());
        let (_, refreshed_user, _) = store
            .validate_api_key(&token)
            .await
            .expect("refreshed validation succeeds")
            .expect("key remains valid");
        assert!(refreshed_user.last_login_at.is_some());
    }

    #[tokio::test]
    async fn persisted_auth_policy_corruption_returns_error_without_caching() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db.clone(), log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("corrupt-policy", "password", UserRole::User, &[])
            .await
            .expect("user creates");
        let (api_key, token) = store
            .create_api_key(&user.id, "corrupt-policy-key", None)
            .await
            .expect("key creates");

        let api_key_cases: Vec<(&str, SeaValue, SeaValue)> = vec![
            (
                "model_limits",
                SeaValue::Int(Some(7)),
                "[]".to_string().into(),
            ),
            (
                "ip_whitelist",
                r#"["not-an-ip"]"#.to_string().into(),
                "[]".to_string().into(),
            ),
            (
                "allowed_groups",
                "{".to_string().into(),
                "[]".to_string().into(),
            ),
            (
                "transforms",
                "{".to_string().into(),
                "[]".to_string().into(),
            ),
            (
                "model_redirects",
                r#"[{"pattern":"(","replace":"target"}]"#.to_string().into(),
                "[]".to_string().into(),
            ),
            ("enabled", SeaValue::Int(Some(2)), SeaValue::Int(Some(1))),
            (
                "sub_account_enabled",
                "not-an-integer".to_string().into(),
                SeaValue::Int(Some(0)),
            ),
            (
                "model_limits_enabled",
                SeaValue::Int(Some(2)),
                SeaValue::Int(Some(0)),
            ),
            (
                "reasoning_envelope_enabled",
                SeaValue::Int(Some(2)),
                SeaValue::Int(Some(1)),
            ),
            (
                "request_capture_mode",
                "unsupported".to_string().into(),
                "off".to_string().into(),
            ),
        ];

        for (column, invalid, valid) in api_key_cases {
            db.write()
                .await
                .execute(db.stmt(
                    &format!("UPDATE api_keys SET {column} = $1 WHERE id = $2"),
                    vec![invalid, api_key.id.clone().into()],
                ))
                .await
                .expect("corrupt API-key policy column");

            let error = store
                .validate_api_key(&token)
                .await
                .expect_err("corrupt API-key policy must fail validation");
            assert!(error.contains(column), "{column}: {error}");
            assert!(store.api_key_cache.get(&token).is_none());

            db.write()
                .await
                .execute(db.stmt(
                    &format!("UPDATE api_keys SET {column} = $1 WHERE id = $2"),
                    vec![valid, api_key.id.clone().into()],
                ))
                .await
                .expect("restore API-key policy column");
        }

        for (column, invalid, valid) in [
            (
                "allowed_groups",
                SeaValue::Int(Some(7)),
                "[]".to_string().into(),
            ),
            ("enabled", SeaValue::Int(Some(2)), SeaValue::Int(Some(1))),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    &format!("UPDATE users SET {column} = $1 WHERE id = $2"),
                    vec![invalid, user.id.clone().into()],
                ))
                .await
                .expect("corrupt user policy column");

            let error = store
                .validate_api_key(&token)
                .await
                .expect_err("corrupt user policy must fail validation");
            assert!(error.contains(column), "{column}: {error}");
            assert!(store.api_key_cache.get(&token).is_none());

            db.write()
                .await
                .execute(db.stmt(
                    &format!("UPDATE users SET {column} = $1 WHERE id = $2"),
                    vec![valid, user.id.clone().into()],
                ))
                .await
                .expect("restore user policy column");
        }

        let last_used_at = db
            .read()
            .query_one(db.stmt(
                "SELECT last_used_at FROM api_keys WHERE id = $1",
                vec![api_key.id.into()],
            ))
            .await
            .expect("last-used query")
            .expect("key row exists")
            .try_get::<Option<String>>("", "last_used_at")
            .expect("last-used decodes");
        assert!(last_used_at.is_none());

        store
            .validate_api_key(&token)
            .await
            .expect("restored policy validates")
            .expect("restored key authenticates");
        assert!(store.api_key_cache.get(&token).is_some());
    }

    #[tokio::test]
    async fn delete_user_uses_reverse_invalidation_without_returning_key_ids() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db.clone(), log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("delete-cache", "password", UserRole::User, &[])
            .await
            .expect("user creates");
        let mut tokens = Vec::new();
        for name in ["first", "second", "third"] {
            let (_, token) = store
                .create_api_key(&user.id, name, None)
                .await
                .expect("key creates");
            store
                .validate_api_key(&token)
                .await
                .expect("key validates")
                .expect("key exists");
            tokens.push(token);
        }
        store
            .get_user_balance(&user.id)
            .await
            .expect("balance reads")
            .expect("balance exists");
        assert!(
            tokens
                .iter()
                .all(|token| store.api_key_cache.get(token).is_some())
        );
        assert!(store.balance_cache.get(&user.id).is_some());

        store.delete_user(&user.id).await.expect("user deletes");

        assert!(
            tokens
                .iter()
                .all(|token| store.api_key_cache.get(token).is_none())
        );
        assert!(store.balance_cache.get(&user.id).is_none());
        assert!(store.get_user_by_id(&user.id).await.unwrap().is_none());
        assert!(store.delete_user(&user.id).await.is_err());
    }

    #[tokio::test]
    async fn session_cleanup_is_indexed_set_delete_and_runs_at_store_startup() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let index = db
            .read()
            .query_one(db.stmt(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name = $1",
                vec!["idx_sessions_expires_at".into()],
            ))
            .await
            .expect("index query succeeds");
        assert!(index.is_some());

        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db.clone(), log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("session-cleanup", "password", UserRole::User, &[])
            .await
            .expect("user creates");
        store
            .create_session(&user.id, -1)
            .await
            .expect("expired session creates");
        let future = store
            .create_session(&user.id, 1)
            .await
            .expect("future session creates");

        let (second_broadcast, _) = tokio::sync::broadcast::channel(4);
        let restarted = UserStore::new(db.clone(), second_broadcast)
            .await
            .expect("restarted store creates");
        let remaining = db
            .read()
            .query_one(db.stmt("SELECT COUNT(*) AS count FROM sessions", vec![]))
            .await
            .expect("session count succeeds")
            .expect("count row exists");
        let remaining: i64 = remaining.try_get("", "count").expect("count decodes");
        assert_eq!(remaining, 1);
        assert!(
            restarted
                .get_session_by_token(&future.token)
                .await
                .expect("future session reads")
                .is_some()
        );

        db.write()
            .await
            .execute(db.stmt(
                "UPDATE sessions SET expires_at = $1 WHERE token = $2",
                vec![
                    (Utc::now() - chrono::Duration::seconds(1))
                        .to_rfc3339()
                        .into(),
                    future.token.into(),
                ],
            ))
            .await
            .expect("session expires");
        assert_eq!(
            restarted
                .cleanup_expired_sessions()
                .await
                .expect("cleanup succeeds"),
            1
        );
    }

    #[tokio::test]
    async fn concurrent_registration_creates_exactly_one_first_super_admin() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db, log_broadcast)
            .await
            .expect("store creates");
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut tasks = Vec::new();
        for username in ["first-racer", "second-racer"] {
            let store = store.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                store
                    .register_user_atomic(username, "password123", false)
                    .await
            }));
        }

        let mut users = Vec::new();
        let mut disabled = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(user) => users.push(user),
                Err(RegisterUserError::RegistrationDisabled) => disabled += 1,
                Err(error) => panic!("unexpected registration result: {error:?}"),
            }
        }
        assert_eq!(users.len(), 1);
        assert_eq!(disabled, 1);
        assert_eq!(users[0].role, UserRole::SuperAdmin);
        assert_eq!(store.user_count().await.unwrap(), 1);
    }

    fn limited_api_key_input(name: String) -> CreateApiKeyInput {
        CreateApiKeyInput {
            name,
            expires_in_days: None,
            sub_account_enabled: false,
            sub_account_balance_nano_usd: None,
            model_limits_enabled: false,
            model_limits: Vec::new(),
            ip_whitelist: Vec::new(),
            allowed_groups: Vec::new(),
            max_multiplier: None,
            transforms: Vec::new(),
            model_redirects: Vec::new(),
            reasoning_envelope_enabled: true,
            request_capture_mode: RequestCaptureMode::Off,
        }
    }

    #[tokio::test]
    async fn concurrent_api_key_creation_never_exceeds_user_limit() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db, log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("key-limit-user", "password123", UserRole::User, &[])
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(6));
        let mut tasks = Vec::new();
        for index in 0..6 {
            let store = store.clone();
            let barrier = barrier.clone();
            let user_id = user.id.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                store
                    .create_api_key_extended_with_limit(
                        &user_id,
                        limited_api_key_input(format!("key-{index}")),
                        false,
                        2,
                    )
                    .await
            }));
        }

        let mut created = 0;
        let mut rejected = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(_) => created += 1,
                Err(CreateApiKeyWithLimitError::LimitReached { limit: 2 }) => rejected += 1,
                Err(error) => panic!("unexpected key creation result: {error:?}"),
            }
        }
        assert_eq!(created, 2);
        assert_eq!(rejected, 4);
        assert_eq!(store.count_user_api_keys(&user.id).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn admin_user_update_rolls_back_ordinary_fields_when_ledger_insert_fails() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db.clone(), log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("atomic-before", "password123", UserRole::User, &[])
            .await
            .unwrap();
        db.write()
            .await
            .execute(db.stmt(
                "CREATE TRIGGER fail_admin_adjustment
                 BEFORE INSERT ON billing_ledger
                 WHEN NEW.kind = 'admin_adjustment'
                 BEGIN SELECT RAISE(FAIL, 'ledger blocked'); END",
                vec![],
            ))
            .await
            .unwrap();

        let result = store
            .admin_update_user_atomic(
                &user.id,
                AdminUpdateUserInput {
                    username: Some("atomic-after".to_string()),
                    balance_nano_usd: Some("50".to_string()),
                    ..AdminUpdateUserInput::default()
                },
                "admin-1",
            )
            .await;
        assert!(result.is_err());

        let unchanged = store.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(unchanged.username, "atomic-before");
        assert_eq!(unchanged.balance_nano_usd, "0");
    }

    #[tokio::test]
    async fn batch_delete_settles_multiple_keys_for_one_user() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_broadcast, _) = tokio::sync::broadcast::channel(4);
        let store = UserStore::new(db.clone(), log_broadcast)
            .await
            .expect("store creates");
        let user = store
            .create_user("batch-settlement", "password", UserRole::User, &[])
            .await
            .expect("user creates");
        let (first, _) = store
            .create_api_key(&user.id, "first", None)
            .await
            .expect("first key creates");
        let (second, _) = store
            .create_api_key(&user.id, "second", None)
            .await
            .expect("second key creates");
        db.write()
            .await
            .execute(db.stmt(
                "UPDATE users SET balance_nano_usd = '100' WHERE id = $1",
                vec![user.id.clone().into()],
            ))
            .await
            .expect("user balance seeds");
        for (id, balance) in [(&first.id, "5"), (&second.id, "7")] {
            db.write()
                .await
                .execute(db.stmt(
                    "UPDATE api_keys
                     SET sub_account_enabled = 1, sub_account_balance_nano = $1
                     WHERE id = $2",
                    vec![balance.into(), id.as_str().into()],
                ))
                .await
                .expect("key balance seeds");
        }

        assert_eq!(
            store
                .batch_delete_api_keys(&[second.id.clone(), first.id.clone()])
                .await
                .expect("batch deletes"),
            2
        );
        let user_row = db
            .read()
            .query_one(db.stmt(
                "SELECT balance_nano_usd FROM users WHERE id = $1",
                vec![user.id.clone().into()],
            ))
            .await
            .expect("user query")
            .expect("user remains");
        assert_eq!(
            user_row
                .try_get::<String>("", "balance_nano_usd")
                .expect("balance decodes"),
            "112"
        );
        let ledger_rows = db
            .read()
            .query_all(db.stmt(
                "SELECT delta_nano_usd FROM billing_ledger
                 WHERE user_id = $1 AND kind = 'sub_account_delete_settlement'",
                vec![user.id.into()],
            ))
            .await
            .expect("ledger query");
        let mut deltas = ledger_rows
            .into_iter()
            .map(|row| {
                row.try_get::<String>("", "delta_nano_usd")
                    .expect("delta decodes")
            })
            .collect::<Vec<_>>();
        deltas.sort();
        assert_eq!(deltas, vec!["5".to_string(), "7".to_string()]);
    }

    #[test]
    fn ip_whitelist_accepts_and_canonicalizes_addresses_and_networks() {
        let values = canonicalize_ip_whitelist(&[
            " 2001:0db8::1 ".to_string(),
            "192.0.2.7".to_string(),
            "192.0.2.0/24".to_string(),
            "192.0.2.7".to_string(),
        ])
        .expect("valid whitelist");
        assert_eq!(
            values,
            vec![
                "192.0.2.0/24".to_string(),
                "192.0.2.7".to_string(),
                "2001:db8::1".to_string(),
            ]
        );
        assert!(canonicalize_ip_whitelist(&["not-an-ip".to_string()]).is_err());
    }

    #[test]
    fn sanitize_api_key_transforms_drops_disallowed_rules() {
        let transforms = vec![TransformRuleConfig {
            transform: "set_field".to_string(),
            enabled: true,
            models: Some(vec!["gpt-5.4-fast".to_string()]),
            phase: Phase::Request,
            config: json!({
                "path": "service_tier",
                "value": "priority"
            }),
        }];

        let sanitized = sanitize_api_key_transforms(transforms, false);
        assert!(sanitized.is_empty());
    }

    #[test]
    fn validate_api_key_transforms_allows_image_compression() {
        let transforms = vec![TransformRuleConfig {
            transform: "compress_user_message_images".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Request,
            config: json!({
                "max_edge_px": 1024,
                "jpeg_quality": 80,
                "skip_if_smaller": true
            }),
        }];

        assert!(validate_api_key_transforms(&transforms, false).is_ok());
    }

    #[test]
    fn validate_api_key_transforms_allows_openai_tool_cache_breakpoints() {
        let transforms = vec![TransformRuleConfig {
            transform: "auto_cache_openai_tool_use".to_string(),
            enabled: true,
            models: Some(vec!["gpt-5.6*".to_string()]),
            phase: Phase::Request,
            config: json!({}),
        }];

        assert!(validate_api_key_transforms(&transforms, false).is_ok());
    }

    #[test]
    fn sanitize_api_key_transforms_canonicalizes_allowed_aliases() {
        let transforms = vec![TransformRuleConfig {
            transform: "remove_anthropic_billing_header".to_string(),
            enabled: true,
            models: None,
            phase: Phase::Request,
            config: json!({}),
        }];

        let sanitized = sanitize_api_key_transforms(transforms, false);

        assert_eq!(sanitized.len(), 1);
        assert_eq!(sanitized[0].transform, "strip_anthropic_billing_header");
    }

    #[test]
    fn validate_api_key_transforms_allows_new_response_transforms() {
        let transforms = vec![
            TransformRuleConfig {
                transform: "plaintext_reasoning_to_summary".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({}),
            },
            TransformRuleConfig {
                transform: "strip_encrypted_reasoning".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({}),
            },
            TransformRuleConfig {
                transform: "assistant_markdown_images_to_output".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({}),
            },
            TransformRuleConfig {
                transform: "reasoning_content_delta".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({}),
            },
            TransformRuleConfig {
                transform: "reasoning_summary_to_raw_cot".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({}),
            },
            TransformRuleConfig {
                transform: "assistant_output_images_to_markdown".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({}),
            },
            TransformRuleConfig {
                transform: "compress_assistant_output_images".to_string(),
                enabled: true,
                models: None,
                phase: Phase::Response,
                config: json!({
                    "max_edge_px": 1024,
                    "jpeg_quality": 80,
                    "skip_if_smaller": true
                }),
            },
        ];

        assert!(validate_api_key_transforms(&transforms, false).is_ok());
    }

    #[test]
    fn sanitize_api_key_transforms_preserves_disallowed_rules_for_admin() {
        let transforms = vec![TransformRuleConfig {
            transform: "set_field".to_string(),
            enabled: true,
            models: Some(vec!["gpt-5.4-fast".to_string()]),
            phase: Phase::Request,
            config: json!({
                "path": "service_tier",
                "value": "priority"
            }),
        }];

        let sanitized = sanitize_api_key_transforms(transforms.clone(), true);
        assert_eq!(sanitized.len(), 1);
        assert_eq!(sanitized[0].transform, transforms[0].transform);
        assert_eq!(sanitized[0].enabled, transforms[0].enabled);
        assert_eq!(sanitized[0].models, transforms[0].models);
        assert_eq!(sanitized[0].phase as u8, transforms[0].phase as u8);
        assert_eq!(sanitized[0].config, transforms[0].config);
    }

    #[test]
    fn allowed_groups_json_compatibility_does_not_accept_corruption() {
        for raw in [None, Some(""), Some("   "), Some("null"), Some("[]")] {
            assert!(
                parse_allowed_groups_json(raw, "allowed_groups")
                    .expect("compatibility value parses")
                    .is_empty()
            );
        }
        for raw in ["not-json", "{}", r#"["group", 1]"#] {
            assert!(parse_allowed_groups_json(Some(raw), "allowed_groups").is_err());
        }
        assert_eq!(
            parse_allowed_groups_json(Some(r#"[" Beta ","alpha","ALPHA",""]"#), "allowed_groups",)
                .expect("valid groups parse"),
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert_eq!(
            serialize_allowed_groups_json(&[
                " Beta ".to_string(),
                "alpha".to_string(),
                "ALPHA".to_string(),
            ])
            .expect("serialize groups"),
            r#"["alpha","beta"]"#
        );
    }

    #[test]
    fn api_key_allowed_groups_must_stay_within_non_empty_user_ceiling() {
        assert!(
            validate_api_key_allowed_groups_subset(
                &["team-a".to_string()],
                &["TEAM-A".to_string()]
            )
            .is_ok()
        );
        assert!(validate_api_key_allowed_groups_subset(&[], &["team-b".to_string()]).is_ok());

        let err = validate_api_key_allowed_groups_subset(
            &["team-a".to_string()],
            &["team-b".to_string()],
        )
        .expect_err("expected subset validation failure");
        assert!(err.contains("invalid_request"));
        assert!(err.contains("subset"));
    }
}

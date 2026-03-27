use super::api_keys::{
    ApiKeyCreatedResponse, ApiKeyResponse, CreateApiKeyRequest, UpdateApiKeyRequest,
    canonicalize_dashboard_api_key_allowed_groups,
};
use super::providers::{
    build_models_list_url, provider_has_billable_pricing, provider_pricing_model,
};
use super::users::{
    CreateUserRequest, UpdateUserRequest, canonicalize_dashboard_user_allowed_groups,
};
use crate::dashboard_handlers::auth::UserResponse;
use crate::db::DbPool;
use crate::migration::Migrator;
use crate::monoize_routing::{
    CreateMonoizeProviderInput, MonoizeChannel, MonoizeModelEntry, MonoizeProvider,
    MonoizeProviderType, MonoizeRoutingStore, UpdateMonoizeProviderInput,
};
use crate::providers::ProviderStore;
use crate::users::{
    CreateApiKeyInput, ModelRedirectRule, RequestLogApiKey, RequestLogBilling, RequestLogChannel,
    RequestLogError, RequestLogProvider, RequestLogRow, RequestLogTiming, RequestLogTokens,
    RequestLogUser, UpdateApiKeyInput, User, UserRole, UserStore,
};
use sea_orm::ConnectionTrait;
use sea_orm_migration::MigratorTrait;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn build_models_list_url_adds_v1_when_missing() {
    assert_eq!(
        build_models_list_url("https://openrouter.ai/api"),
        "https://openrouter.ai/api/v1/models"
    );
}

#[test]
fn build_models_list_url_avoids_duplicate_v1_suffix() {
    assert_eq!(
        build_models_list_url("https://openrouter.ai/api/v1"),
        "https://openrouter.ai/api/v1/models"
    );
    assert_eq!(
        build_models_list_url("https://openrouter.ai/api/v1/"),
        "https://openrouter.ai/api/v1/models"
    );
}

#[test]
fn provider_pricing_model_uses_redirect_when_present() {
    let entry = MonoizeModelEntry {
        redirect: Some("  gpt-5-target  ".to_string()),
        multiplier: 1.0,
    };
    assert_eq!(
        provider_pricing_model("gpt-5-logical", &entry),
        "gpt-5-target"
    );
}

#[test]
fn provider_pricing_model_falls_back_to_logical_when_redirect_blank() {
    let entry = MonoizeModelEntry {
        redirect: Some("   ".to_string()),
        multiplier: 1.0,
    };
    assert_eq!(
        provider_pricing_model("gpt-5-logical", &entry),
        "gpt-5-logical"
    );
}

#[test]
fn provider_has_billable_pricing_accepts_logical_fallback_when_redirect_target_is_unpriced() {
    let entry = MonoizeModelEntry {
        redirect: Some("gpt-5-target".to_string()),
        multiplier: 1.0,
    };
    let priced_ids = std::collections::HashSet::from(["gpt-5-logical".to_string()]);
    let reasoning_suffix_map = HashMap::new();

    assert!(provider_has_billable_pricing(
        "gpt-5-logical",
        &entry,
        &priced_ids,
        &reasoning_suffix_map,
    ));
}

#[test]
fn provider_has_billable_pricing_strips_reasoning_suffix_before_lookup() {
    let entry = MonoizeModelEntry {
        redirect: None,
        multiplier: 1.0,
    };
    let priced_ids = std::collections::HashSet::from(["gpt-5-mini".to_string()]);
    let reasoning_suffix_map = HashMap::from([("-thinking".to_string(), "high".to_string())]);

    assert!(provider_has_billable_pricing(
        "gpt-5-mini-thinking",
        &entry,
        &priced_ids,
        &reasoning_suffix_map,
    ));
}

#[test]
fn dashboard_create_provider_groups_default_to_public() {
    let body: CreateMonoizeProviderInput = serde_json::from_value(json!({
        "name": "OpenAI",
        "provider_type": "responses",
        "models": {
            "gpt-5": {
                "redirect": null,
                "multiplier": 1.0
            }
        },
        "channels": [
            {
                "name": "public",
                "base_url": "https://example.com/public",
                "api_key": "secret"
            },
            {
                "name": "restricted",
                "base_url": "https://example.com/restricted",
                "api_key": "secret"
            }
        ]
    }))
    .expect("payload deserializes");

    assert!(body.groups.is_empty());
}

#[test]
fn dashboard_update_provider_groups_are_partial() {
    let body: UpdateMonoizeProviderInput = serde_json::from_value(json!({
        "channels": [
            {
                "id": "mono_ch_existing",
                "name": "existing",
                "base_url": "https://example.com/existing"
            }
        ]
    }))
    .expect("payload deserializes");

    assert!(body.groups.is_none());
}

#[test]
fn dashboard_provider_response_includes_groups_and_channel_hides_api_key() {
    let channel = MonoizeChannel {
        id: "mono_ch_123".to_string(),
        name: "primary".to_string(),
        base_url: "https://example.com".to_string(),
        api_key: "secret".to_string(),
        weight: 1,
        enabled: true,
        passive_failure_count_threshold_override: None,
        passive_cooldown_seconds_override: None,
        passive_window_seconds_override: None,
        passive_rate_limit_cooldown_seconds_override: None,
        _healthy: None,
        _last_success_at: None,
        _health_status: None,
    };

    let provider = MonoizeProvider {
        id: "mono_provider_123".to_string(),
        name: "provider".to_string(),
        provider_type: MonoizeProviderType::Responses,
        models: HashMap::new(),
        channels: vec![channel],
        max_retries: -1,
        channel_max_retries: 0,
        channel_retry_interval_ms: 0,
        circuit_breaker_enabled: true,
        per_model_circuit_break: false,
        transforms: Vec::new(),
        api_type_overrides: Vec::new(),
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        request_timeout_ms_override: None,
        extra_fields_whitelist: None,
        groups: vec!["alpha".to_string(), "beta".to_string()],
        enabled: true,
        priority: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let value = serde_json::to_value(&provider).expect("provider serializes");
    let object = value.as_object().expect("provider object");
    let channels = object
        .get("channels")
        .and_then(|value| value.as_array())
        .expect("channels array");
    let channel_object = channels[0].as_object().expect("channel object");

    assert_eq!(object.get("groups"), Some(&json!(["alpha", "beta"])));
    assert!(!channel_object.contains_key("api_key"));
    assert!(!channel_object.contains_key("groups"));
}

#[tokio::test]
async fn dashboard_provider_groups_round_trip_through_store_and_update_preserves_or_clears_them() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let store = MonoizeRoutingStore::new(db).await.expect("store creates");

    let create_body: CreateMonoizeProviderInput = serde_json::from_value(json!({
        "name": "OpenAI",
        "provider_type": "responses",
        "groups": [" Beta ", "alpha", "ALPHA", ""],
        "models": {
            "gpt-5": {
                "redirect": null,
                "multiplier": 1.0
            }
        },
        "channels": [
            {
                "name": "primary",
                "base_url": "https://example.com",
                "api_key": "secret"
            }
        ]
    }))
    .expect("create payload deserializes");

    let created = store
        .create_provider(create_body)
        .await
        .expect("provider created");
    let channel_id = created.channels[0].id.clone();

    assert_eq!(
        created.groups,
        vec!["alpha".to_string(), "beta".to_string()]
    );
    assert_eq!(created.channels[0].api_key, "secret");

    let update_body: UpdateMonoizeProviderInput = serde_json::from_value(json!({
        "channels": [
            {
                "id": channel_id,
                "name": "primary",
                "base_url": "https://example.com",
                "api_key": ""
            }
        ]
    }))
    .expect("update payload deserializes");

    let updated = store
        .update_provider(&created.id, update_body)
        .await
        .expect("provider updated");

    assert_eq!(
        updated.groups,
        vec!["alpha".to_string(), "beta".to_string()]
    );
    assert_eq!(updated.channels[0].api_key, "secret");

    let cleared = store
        .update_provider(
            &created.id,
            serde_json::from_value(json!({
                "groups": []
            }))
            .expect("clear payload deserializes"),
        )
        .await
        .expect("provider groups cleared");

    assert!(cleared.groups.is_empty());
}

#[test]
fn dashboard_create_user_allowed_groups_default_to_unrestricted_and_canonicalize() {
    let mut body: CreateUserRequest = serde_json::from_value(json!({
        "username": "alice",
        "password": "password123",
        "role": "user"
    }))
    .expect("payload deserializes");

    assert!(body.allowed_groups.is_empty());

    body.allowed_groups = vec![
        " Beta ".to_string(),
        "alpha".to_string(),
        "ALPHA".to_string(),
        "".to_string(),
    ];
    canonicalize_dashboard_user_allowed_groups(&mut body.allowed_groups);

    assert_eq!(
        body.allowed_groups,
        vec!["alpha".to_string(), "beta".to_string()]
    );
}

#[test]
fn dashboard_create_api_key_allowed_groups_default_to_inherit_and_canonicalize() {
    let mut body: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "default key"
    }))
    .expect("payload deserializes");

    assert!(body.allowed_groups.is_empty());

    body.allowed_groups = vec![
        " Beta ".to_string(),
        "alpha".to_string(),
        "ALPHA".to_string(),
        "".to_string(),
    ];
    canonicalize_dashboard_api_key_allowed_groups(&mut body.allowed_groups);

    assert_eq!(
        body.allowed_groups,
        vec!["alpha".to_string(), "beta".to_string()]
    );
}

#[test]
fn dashboard_update_api_key_allowed_groups_is_partial_and_canonicalized_when_present() {
    let omitted: UpdateApiKeyRequest = serde_json::from_value(json!({
        "name": "renamed key"
    }))
    .expect("payload deserializes");
    assert!(omitted.allowed_groups.is_none());

    let mut present: UpdateApiKeyRequest = serde_json::from_value(json!({
        "allowed_groups": [" Beta ", "alpha", "ALPHA", ""]
    }))
    .expect("payload deserializes");
    canonicalize_dashboard_api_key_allowed_groups(
        present
            .allowed_groups
            .as_mut()
            .expect("allowed_groups present"),
    );

    assert_eq!(
        present.allowed_groups,
        Some(vec!["alpha".to_string(), "beta".to_string()])
    );
}

#[tokio::test]
async fn dashboard_user_allowed_groups_round_trip_through_store_and_response() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");

    let mut create_body: CreateUserRequest = serde_json::from_value(json!({
        "username": "alice",
        "password": "password123",
        "role": "user",
        "allowed_groups": [" Beta ", "alpha", "ALPHA", ""]
    }))
    .expect("create payload deserializes");
    canonicalize_dashboard_user_allowed_groups(&mut create_body.allowed_groups);

    let created = store
        .create_user(
            &create_body.username,
            &create_body.password,
            UserRole::User,
            &create_body.allowed_groups,
        )
        .await
        .expect("user created");

    assert_eq!(
        created.allowed_groups,
        vec!["alpha".to_string(), "beta".to_string()]
    );

    let mut update_body: UpdateUserRequest = serde_json::from_value(json!({
        "allowed_groups": [" Gamma ", "alpha", "ALPHA", ""]
    }))
    .expect("update payload deserializes");

    let groups = update_body
        .allowed_groups
        .as_mut()
        .expect("allowed_groups present");
    canonicalize_dashboard_user_allowed_groups(groups);

    store
        .update_user(
            &created.id,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            update_body.allowed_groups.as_deref(),
        )
        .await
        .expect("user updated");

    let fetched = store
        .get_user_by_id(&created.id)
        .await
        .expect("lookup succeeds")
        .expect("user exists");
    assert_eq!(
        fetched.allowed_groups,
        vec!["alpha".to_string(), "gamma".to_string()]
    );

    let listed = store.list_users().await.expect("list succeeds");
    let listed_user = listed
        .into_iter()
        .find(|user| user.id == created.id)
        .expect("listed user exists");
    assert_eq!(
        listed_user.allowed_groups,
        vec!["alpha".to_string(), "gamma".to_string()]
    );

    let response = serde_json::to_value(UserResponse::from(fetched)).expect("response serializes");
    assert_eq!(
        response.get("allowed_groups"),
        Some(&json!(["alpha", "gamma"]))
    );
}

#[test]
fn user_response_serializes_allowed_groups() {
    let user = User {
        id: "user-1".to_string(),
        username: "alice".to_string(),
        password_hash: "hash".to_string(),
        role: UserRole::User,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_login_at: None,
        enabled: true,
        balance_nano_usd: "0".to_string(),
        balance_unlimited: false,
        email: None,
        allowed_groups: vec!["alpha".to_string(), "beta".to_string()],
    };

    let value = serde_json::to_value(UserResponse::from(user)).expect("response serializes");
    assert_eq!(value.get("allowed_groups"), Some(&json!(["alpha", "beta"])));
}

#[tokio::test]
async fn dashboard_api_key_allowed_groups_round_trip_through_store_and_responses() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");

    let user = store
        .create_user(
            "alice",
            "password123",
            UserRole::User,
            &["alpha".to_string(), "beta".to_string()],
        )
        .await
        .expect("user created");

    let mut create_body: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "dashboard key",
        "allowed_groups": [" Beta ", "alpha", "ALPHA", ""]
    }))
    .expect("create payload deserializes");
    canonicalize_dashboard_api_key_allowed_groups(&mut create_body.allowed_groups);

    let (created, key) = store
        .create_api_key_extended(
            &user.id,
            CreateApiKeyInput {
                name: create_body.name,
                expires_in_days: create_body.expires_in_days,
                quota: create_body.quota,
                quota_unlimited: create_body.quota_unlimited,
                model_limits_enabled: create_body.model_limits_enabled,
                model_limits: create_body.model_limits,
                ip_whitelist: create_body.ip_whitelist,
                allowed_groups: create_body.allowed_groups,
                max_multiplier: create_body.max_multiplier,
                transforms: create_body.transforms,
                model_redirects: create_body.model_redirects,
            },
            false,
        )
        .await
        .expect("api key created");

    assert_eq!(
        created.allowed_groups,
        vec!["alpha".to_string(), "beta".to_string()]
    );

    let created_value = serde_json::to_value(ApiKeyCreatedResponse {
        id: created.id.clone(),
        name: created.name.clone(),
        key,
        key_prefix: created.key_prefix.clone(),
        created_at: created.created_at.to_rfc3339(),
        expires_at: created.expires_at.map(|date| date.to_rfc3339()),
        quota_remaining: created.quota_remaining,
        quota_unlimited: created.quota_unlimited,
        model_limits_enabled: created.model_limits_enabled,
        model_limits: created.model_limits.clone(),
        ip_whitelist: created.ip_whitelist.clone(),
        allowed_groups: created.allowed_groups.clone(),
        max_multiplier: created.max_multiplier,
        transforms: created.transforms.clone(),
        model_redirects: created.model_redirects.clone(),
    })
    .expect("created response serializes");
    assert_eq!(
        created_value.get("allowed_groups"),
        Some(&json!(["alpha", "beta"]))
    );

    let mut update_body: UpdateApiKeyRequest = serde_json::from_value(json!({
        "allowed_groups": [" Beta ", ""]
    }))
    .expect("update payload deserializes");
    canonicalize_dashboard_api_key_allowed_groups(
        update_body
            .allowed_groups
            .as_mut()
            .expect("allowed_groups present"),
    );

    let updated = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                quota: None,
                quota_unlimited: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                allowed_groups: update_body.allowed_groups,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect("api key updated");

    assert_eq!(updated.allowed_groups, vec!["beta".to_string()]);

    let fetched = store
        .get_api_key_by_id(&updated.id)
        .await
        .expect("lookup succeeds")
        .expect("api key exists");
    assert_eq!(fetched.allowed_groups, vec!["beta".to_string()]);

    let listed_key = store
        .list_user_api_keys(&user.id)
        .await
        .expect("list succeeds")
        .into_iter()
        .find(|api_key| api_key.id == updated.id)
        .expect("listed api key exists");
    assert_eq!(listed_key.allowed_groups, vec!["beta".to_string()]);

    let response_value = serde_json::to_value(ApiKeyResponse {
        id: fetched.id,
        name: fetched.name,
        key_prefix: fetched.key_prefix,
        key: fetched.key,
        created_at: fetched.created_at.to_rfc3339(),
        expires_at: fetched.expires_at.map(|date| date.to_rfc3339()),
        last_used_at: fetched.last_used_at.map(|date| date.to_rfc3339()),
        enabled: fetched.enabled,
        quota_remaining: fetched.quota_remaining,
        quota_unlimited: fetched.quota_unlimited,
        model_limits_enabled: fetched.model_limits_enabled,
        model_limits: fetched.model_limits,
        ip_whitelist: fetched.ip_whitelist,
        allowed_groups: fetched.allowed_groups,
        max_multiplier: fetched.max_multiplier,
        transforms: fetched.transforms,
        model_redirects: fetched.model_redirects,
    })
    .expect("response serializes");
    assert_eq!(response_value.get("allowed_groups"), Some(&json!(["beta"])));
}

#[tokio::test]
async fn dashboard_api_key_allowed_groups_enforces_user_ceiling() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");

    let restricted_user = store
        .create_user(
            "restricted",
            "password123",
            UserRole::User,
            &["alpha".to_string()],
        )
        .await
        .expect("restricted user created");

    let mut invalid_create_body: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "invalid key",
        "allowed_groups": [" beta "]
    }))
    .expect("create payload deserializes");
    canonicalize_dashboard_api_key_allowed_groups(&mut invalid_create_body.allowed_groups);

    let create_err = store
        .create_api_key_extended(
            &restricted_user.id,
            CreateApiKeyInput {
                name: invalid_create_body.name,
                expires_in_days: invalid_create_body.expires_in_days,
                quota: invalid_create_body.quota,
                quota_unlimited: invalid_create_body.quota_unlimited,
                model_limits_enabled: invalid_create_body.model_limits_enabled,
                model_limits: invalid_create_body.model_limits,
                ip_whitelist: invalid_create_body.ip_whitelist,
                allowed_groups: invalid_create_body.allowed_groups,
                max_multiplier: invalid_create_body.max_multiplier,
                transforms: invalid_create_body.transforms,
                model_redirects: invalid_create_body.model_redirects,
            },
            false,
        )
        .await
        .expect_err("create should reject groups outside user ceiling");
    assert!(create_err.contains("invalid_request"));
    assert!(create_err.contains("subset"));

    let (created, _) = store
        .create_api_key_extended(
            &restricted_user.id,
            CreateApiKeyInput {
                name: "valid key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
            },
            false,
        )
        .await
        .expect("baseline key created");

    let mut invalid_update_body: UpdateApiKeyRequest = serde_json::from_value(json!({
        "allowed_groups": ["beta"]
    }))
    .expect("update payload deserializes");
    canonicalize_dashboard_api_key_allowed_groups(
        invalid_update_body
            .allowed_groups
            .as_mut()
            .expect("allowed_groups present"),
    );

    let update_err = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                quota: None,
                quota_unlimited: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                allowed_groups: invalid_update_body.allowed_groups,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect_err("update should reject groups outside user ceiling");
    assert!(update_err.contains("invalid_request"));
    assert!(update_err.contains("subset"));

    let unrestricted_user = store
        .create_user("unrestricted", "password123", UserRole::User, &[])
        .await
        .expect("unrestricted user created");
    let unrestricted_key = store
        .create_api_key_extended(
            &unrestricted_user.id,
            CreateApiKeyInput {
                name: "open key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                allowed_groups: vec![" Beta ".to_string()],
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
            },
            false,
        )
        .await
        .expect("unrestricted user may create scoped key");
    assert_eq!(unrestricted_key.0.allowed_groups, vec!["beta".to_string()]);
}

#[tokio::test]
async fn dashboard_api_key_model_redirects_round_trip_and_validate() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");

    let user = store
        .create_user("redirect-user", "password123", UserRole::User, &[])
        .await
        .expect("user created");

    let create_body: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "redirect key",
        "model_redirects": [
            { "pattern": ".*opus.*", "replace": "gpt-5.4" },
            { "pattern": ".*haiku.*", "replace": "gpt-5.4-mini" }
        ]
    }))
    .expect("create payload deserializes");

    let (created, _) = store
        .create_api_key_extended(
            &user.id,
            CreateApiKeyInput {
                name: create_body.name,
                expires_in_days: create_body.expires_in_days,
                quota: create_body.quota,
                quota_unlimited: create_body.quota_unlimited,
                model_limits_enabled: create_body.model_limits_enabled,
                model_limits: create_body.model_limits,
                ip_whitelist: create_body.ip_whitelist,
                allowed_groups: create_body.allowed_groups,
                max_multiplier: create_body.max_multiplier,
                transforms: create_body.transforms,
                model_redirects: create_body.model_redirects,
            },
            false,
        )
        .await
        .expect("api key created");

    assert_eq!(created.model_redirects.len(), 2);
    assert_eq!(created.model_redirects[0].pattern, ".*opus.*");
    assert_eq!(created.model_redirects[0].replace, "gpt-5.4");

    let updated = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                quota: None,
                quota_unlimited: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                allowed_groups: None,
                max_multiplier: None,
                transforms: None,
                model_redirects: Some(vec![ModelRedirectRule {
                    pattern: ".*sonnet.*".to_string(),
                    replace: "gpt-5.4".to_string(),
                }]),
                expires_at: None,
            },
            false,
        )
        .await
        .expect("api key updated");

    assert_eq!(updated.model_redirects.len(), 1);
    assert_eq!(updated.model_redirects[0].pattern, ".*sonnet.*");

    let invalid_create = store
        .create_api_key_extended(
            &user.id,
            CreateApiKeyInput {
                name: "invalid redirect key".to_string(),
                expires_in_days: None,
                quota: None,
                quota_unlimited: true,
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                allowed_groups: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: vec![ModelRedirectRule {
                    pattern: "(".to_string(),
                    replace: "gpt-5.4".to_string(),
                }],
            },
            false,
        )
        .await
        .expect_err("invalid regex should be rejected");

    assert!(invalid_create.starts_with("invalid model redirect pattern:"));
}

#[test]
fn request_log_timing_serializes_compatibility_aliases() {
    let row = RequestLogRow {
        id: "row-1".to_string(),
        request_id: Some("req-1".to_string()),
        created_at: "2026-03-07T00:00:00Z".to_string(),
        status: "success".to_string(),
        is_stream: true,
        model: "gpt-5".to_string(),
        upstream_model: Some("gpt-5-upstream".to_string()),
        request_kind: None,
        reasoning_effort: None,
        request_ip: None,
        tried_providers: None,
        provider: RequestLogProvider {
            id: Some("provider-1".to_string()),
            name: Some("Provider".to_string()),
            multiplier: Some(1.0),
        },
        channel: RequestLogChannel {
            id: Some("channel-1".to_string()),
            name: Some("Channel".to_string()),
        },
        user: RequestLogUser {
            id: "user-1".to_string(),
            username: Some("alice".to_string()),
        },
        api_key: RequestLogApiKey {
            id: Some("key-1".to_string()),
            name: Some("Default".to_string()),
        },
        tokens: RequestLogTokens {
            input: Some(10),
            output: Some(20),
            cache_read: None,
            cache_creation: None,
            tool_prompt: None,
            reasoning: None,
            accepted_prediction: None,
            rejected_prediction: None,
        },
        timing: RequestLogTiming {
            duration_ms: Some(1200),
            ttfb_ms: Some(150),
            duration_ms_alias: Some(1200),
            elapsed_ms: Some(1200),
            latency_ms: Some(1200),
            ttfb_ms_alias: Some(150),
            first_token_ms: Some(150),
            first_token_ms_alias: Some(150),
        },
        billing: RequestLogBilling {
            charge_nano_usd: Some("42".to_string()),
            breakdown: Some(json!({"version": 1})),
        },
        usage: Some(json!({"version": 1})),
        error: RequestLogError {
            code: None,
            message: None,
            http_status: None,
        },
    };

    let value = serde_json::to_value(&row).expect("serializes");
    let timing = value
        .get("timing")
        .and_then(|v| v.as_object())
        .expect("timing object");

    assert_eq!(timing.get("duration_ms"), Some(&json!(1200)));
    assert_eq!(timing.get("durationMs"), Some(&json!(1200)));
    assert_eq!(timing.get("elapsed_ms"), Some(&json!(1200)));
    assert_eq!(timing.get("latency_ms"), Some(&json!(1200)));
    assert_eq!(timing.get("ttfb_ms"), Some(&json!(150)));
    assert_eq!(timing.get("ttfbMs"), Some(&json!(150)));
    assert_eq!(timing.get("first_token_ms"), Some(&json!(150)));
    assert_eq!(timing.get("firstTokenMs"), Some(&json!(150)));
}

#[tokio::test]
async fn provider_store_rejects_invalid_groups_json() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    db.write()
        .await
        .execute(db.stmt(
            r#"INSERT INTO providers
               (id, name, provider_type, base_url, auth_type, auth_value, auth_header_name,
                auth_query_name, capabilities_json, strategy_json, enabled, priority, weight,
                tag, groups_json, balance, created_at, updated_at)
               VALUES ($1, $2, $3, $4, NULL, NULL, NULL, NULL, $5, NULL, 1, 0, 1, NULL, $6, '0', $7, $8)"#,
            vec![
                "prov_bad_groups".into(),
                "Broken Provider".into(),
                "responses".into(),
                "https://example.com".into(),
                "[]".into(),
                "{not-json}".into(),
                "2026-03-07T00:00:00Z".into(),
                "2026-03-07T00:00:00Z".into(),
            ],
        ))
        .await
        .expect("insert provider");

    let store = ProviderStore::new(db).await.expect("store creates");
    let err = store
        .get_provider("prov_bad_groups")
        .await
        .expect_err("invalid groups json should fail");

    assert!(err.contains("invalid groups_json"));
    assert!(err.contains("prov_bad_groups"));
}

#[tokio::test]
async fn sqlite_migration_creates_request_log_retention_indexes() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let rows = db
        .read()
        .query_all(db.stmt(
            "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND tbl_name = 'request_logs' ORDER BY name",
            vec![],
        ))
        .await
        .expect("list sqlite indexes");

    let index_rows: Vec<(String, String)> = rows
        .into_iter()
        .filter_map(|row| {
            Some((
                row.try_get::<String>("", "name").ok()?,
                row.try_get::<String>("", "sql").ok()?,
            ))
        })
        .collect();

    assert!(index_rows.iter().any(|(name, sql)| {
        name == "idx_request_logs_user_created_at"
            && sql.contains("(user_id, created_at_unix_ms DESC)")
    }));
    assert!(index_rows.iter().any(|(name, sql)| {
        name == "idx_request_logs_created_at" && sql.contains("(created_at_unix_ms DESC)")
    }));
}

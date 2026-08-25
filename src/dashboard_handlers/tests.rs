use super::api_keys::{
    ApiKeyCreatedResponse, ApiKeyResponse, CreateApiKeyRequest, UpdateApiKeyRequest,
};
use super::providers::{
    build_dashboard_rate_matrix_cache, build_models_list_url,
    provider_dashboard_rate_matrix_is_complete, provider_pricing_model,
};
use super::users::{CreateUserRequest, UpdateUserRequest};
use crate::billing_rate_store::DbBillingRateRecord;
use crate::dashboard_handlers::auth::UserResponse;
use crate::db::DbPool;
use crate::migration::Migrator;
use crate::monoize_routing::{
    AffinityFailbackMode, CreateMonoizeProviderInput, MonoizeChannel, MonoizeModelEntry,
    MonoizeProvider, MonoizeProviderType, MonoizeRoutingStore, UpdateMonoizeProviderInput,
};
use crate::settings::{PricingProfilePattern, SettingsStore};
use crate::transforms::{Phase, TransformRuleConfig};
use crate::users::{
    CreateApiKeyInput, ModelRedirectRule, RequestLogAffinity, RequestLogApiKey, RequestLogBilling,
    RequestLogChannel, RequestLogError, RequestLogProvider, RequestLogRow, RequestLogTiming,
    RequestLogTokens, RequestLogUser, UpdateApiKeyInput, User, UserRole, UserStore,
};
use sea_orm::ConnectionTrait;
use sea_orm_migration::MigratorTrait;
use serde_json::json;
use std::collections::{HashMap, HashSet};

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
        multiplier: crate::exact_decimal::Multiplier::ONE,
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
        multiplier: crate::exact_decimal::Multiplier::ONE,
    };
    assert_eq!(
        provider_pricing_model("gpt-5-logical", &entry),
        "gpt-5-logical"
    );
}

fn dashboard_rate(id: &str, usage_class: &str, context_tier: Option<&str>) -> DbBillingRateRecord {
    DbBillingRateRecord {
        id: id.to_string(),
        source: "manual".to_string(),
        pricing_profile: "openai".to_string(),
        model_pattern: Some("gpt-test".to_string()),
        provider_type: Some("responses".to_string()),
        rate_kind: "token".to_string(),
        usage_class: usage_class.to_string(),
        unit: "token".to_string(),
        unit_price_nano_usd: "1".to_string(),
        context_tier: context_tier.map(str::to_string),
        service_tier: None,
        modality: None,
        cache_ttl: None,
        match_json: serde_json::json!({}),
        priority: 0,
        enabled: true,
        raw_json: serde_json::json!({}),
        updated_at: chrono::Utc::now(),
    }
}

#[test]
fn provider_dashboard_rate_matrix_requires_complete_tiered_billing_rates() {
    assert!(provider_dashboard_rate_matrix_is_complete(&[
        dashboard_rate("input", "input_uncached", None),
        dashboard_rate("output", "output", None),
    ]));

    let mut tiered = vec![
        dashboard_rate("short-input", "input_uncached", Some("short")),
        dashboard_rate("short-output", "output", Some("short")),
        dashboard_rate("long-input", "input_uncached", Some("long")),
        dashboard_rate("long-output", "output", Some("long")),
    ];
    assert!(!provider_dashboard_rate_matrix_is_complete(&tiered));
    for rate in &mut tiered {
        rate.match_json = serde_json::json!({ "context_threshold_tokens": 128000 });
    }
    assert!(provider_dashboard_rate_matrix_is_complete(&tiered));
    tiered.retain(|rate| rate.id != "long-output");
    assert!(!provider_dashboard_rate_matrix_is_complete(&tiered));
}

#[test]
fn provider_dashboard_rate_matrix_cache_filters_bulk_candidates_in_memory() {
    let pairs = HashSet::from([
        ("gpt-test".to_string(), "responses".to_string()),
        ("gpt-test".to_string(), "messages".to_string()),
        ("metadata-model".to_string(), "messages".to_string()),
    ]);
    let patterns = vec![PricingProfilePattern {
        pattern: "gpt-*".to_string(),
        pricing_profile: "openai".to_string(),
    }];
    let metadata = HashMap::from([("metadata-model".to_string(), "anthropic".to_string())]);
    let mut rates = vec![
        dashboard_rate("openai-input", "input_uncached", None),
        dashboard_rate("openai-output", "output", None),
    ];
    for rate in &mut rates {
        rate.provider_type = Some("responses".to_string());
    }
    let mut metadata_input = dashboard_rate("metadata-input", "input_uncached", None);
    metadata_input.pricing_profile = "anthropic".to_string();
    metadata_input.model_pattern = Some("metadata-*".to_string());
    metadata_input.provider_type = Some("messages".to_string());
    let mut metadata_output = metadata_input.clone();
    metadata_output.id = "metadata-output".to_string();
    metadata_output.usage_class = "output".to_string();
    rates.extend([metadata_input, metadata_output]);

    let cache = build_dashboard_rate_matrix_cache(&pairs, &patterns, &metadata, &rates);
    assert_eq!(
        cache.get(&("gpt-test".to_string(), "responses".to_string())),
        Some(&true)
    );
    assert_eq!(
        cache.get(&("gpt-test".to_string(), "messages".to_string())),
        Some(&false)
    );
    assert_eq!(
        cache.get(&("metadata-model".to_string(), "messages".to_string())),
        Some(&true)
    );
}

#[test]
fn dashboard_create_provider_group_ids_default_to_empty() {
    let body: CreateMonoizeProviderInput = serde_json::from_value(json!({
        "name": "OpenAI",
        "channels": [
            {
                "name": "public",
                "provider_type": "responses",
                "base_url": "https://example.com/public",
                "api_key": "secret",
                "models": { "gpt-5": { "redirect": null, "multiplier": "1" } }
            },
            {
                "name": "restricted",
                "provider_type": "responses",
                "base_url": "https://example.com/restricted",
                "api_key": "secret",
                "models": { "gpt-5": { "redirect": null, "multiplier": "1" } }
            }
        ]
    }))
    .expect("payload deserializes");

    assert!(body.group_ids.is_empty());
}

#[test]
fn dashboard_create_provider_rejects_obsolete_provider_models_field() {
    let result = serde_json::from_value::<CreateMonoizeProviderInput>(json!({
        "name": "OpenAI",
        "models": { "gpt-5": { "redirect": null, "multiplier": "1" } },
        "channels": [{
            "name": "primary",
            "provider_type": "responses",
            "base_url": "https://example.com",
            "api_key": "secret",
            "models": { "gpt-5": { "redirect": null, "multiplier": "1" } }
        }]
    }));

    assert!(
        result.is_err(),
        "provider-level models must not be accepted"
    );
}

#[test]
fn dashboard_update_provider_group_ids_are_partial() {
    let body: UpdateMonoizeProviderInput = serde_json::from_value(json!({
        "channels": [
            {
                "id": "mono_ch_existing",
                "name": "existing",
                "provider_type": "responses",
                "base_url": "https://example.com/existing"
            }
        ]
    }))
    .expect("payload deserializes");

    assert!(body.group_ids.is_none());
}

#[test]
fn dashboard_provider_response_includes_groups_and_channel_hides_api_key() {
    let channel = MonoizeChannel {
        id: "mono_ch_123".to_string(),
        name: "primary".to_string(),
        provider_type: MonoizeProviderType::Responses,
        base_url: "https://example.com".to_string(),
        api_key: "secret".to_string(),
        weight: 1,
        enabled: true,
        passive_failure_count_threshold_override: None,
        passive_cooldown_seconds_override: None,
        passive_window_seconds_override: None,
        passive_rate_limit_cooldown_seconds_override: None,
        models: HashMap::from([(
            "gpt-5".to_string(),
            crate::monoize_routing::MonoizeModelEntry {
                redirect: None,
                multiplier: crate::exact_decimal::Multiplier::ONE,
            },
        )]),
        active_probe_enabled_override: None,
        active_probe_interval_seconds_override: None,
        active_probe_success_threshold_override: None,
        active_probe_model_override: None,
        affinity_enabled_override: None,
        affinity_idle_ttl_seconds_override: None,
        affinity_failback_mode_override: None,
        affinity_failback_delay_seconds_override: None,
        _healthy: None,
        _last_success_at: None,
        _health_status: None,

        proxy_url: None,
        extra_headers: None,
        session_affinity_auto: None,
    };

    let provider = MonoizeProvider {
        id: "mono_provider_123".to_string(),
        name: "provider".to_string(),
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
        strip_cross_protocol_nested_extra: None,
        group_ids: vec!["g-alpha".to_string(), "g-beta".to_string()],
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

    assert_eq!(object.get("group_ids"), Some(&json!(["g-alpha", "g-beta"])));
    assert!(!channel_object.contains_key("api_key"));
    assert!(!channel_object.contains_key("group_ids"));
}

#[tokio::test]
async fn dashboard_provider_group_ids_round_trip_and_empty_selection_binds_default_group() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let default_group_id: String = db
        .read()
        .query_one(db.stmt(
            "SELECT id FROM monoize_groups WHERE is_default = 1",
            vec![],
        ))
        .await
        .expect("default group query succeeds")
        .expect("default group exists")
        .try_get("", "id")
        .expect("default group id decodes");
    for (id, name) in [("g-alpha", "alpha"), ("g-beta", "beta")] {
        db.write()
            .await
            .execute(db.stmt(
                "INSERT INTO monoize_groups (id, name, description, is_default, user_selectable, sort_order, created_at, updated_at) \
                 VALUES ($1, $2, '', 0, 1, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                vec![id.into(), name.into()],
            ))
            .await
            .expect("registry row inserts");
    }

    let store = MonoizeRoutingStore::new(db).await.expect("store creates");

    let create_body: CreateMonoizeProviderInput = serde_json::from_value(json!({
        "name": "OpenAI",
        "group_ids": [" g-beta ", "g-alpha", "g-alpha", ""],
        "channels": [
            {
                "name": "primary",
                "provider_type": "responses",
                "base_url": "https://example.com",
                "api_key": "secret",
                "affinity_enabled_override": true,
                "affinity_idle_ttl_seconds_override": 90,
                "affinity_failback_mode_override": "prefer_higher_priority",
                "affinity_failback_delay_seconds_override": 15,
                "models": { "gpt-5": { "redirect": null, "multiplier": "1" } }
            }
        ]
    }))
    .expect("create payload deserializes");

    let created = store
        .create_provider(create_body)
        .await
        .expect("provider created");
    let channel_id = created.channels[0].id.clone();

    // Trim + dedupe, but preserve the submitted order (no lowercasing of ids).
    assert_eq!(
        created.group_ids,
        vec!["g-beta".to_string(), "g-alpha".to_string()]
    );
    assert_eq!(created.channels[0].api_key, "secret");
    assert_eq!(created.channels[0].affinity_enabled_override, Some(true));
    assert_eq!(
        created.channels[0].affinity_idle_ttl_seconds_override,
        Some(90)
    );
    assert_eq!(
        created.channels[0].affinity_failback_mode_override,
        Some(AffinityFailbackMode::PreferHigherPriority)
    );
    assert_eq!(
        created.channels[0].affinity_failback_delay_seconds_override,
        Some(15)
    );

    let update_body: UpdateMonoizeProviderInput = serde_json::from_value(json!({
        "channels": [
            {
                "id": channel_id,
                "name": "primary",
                "provider_type": "responses",
                "base_url": "https://example.com",
                "api_key": "",
                "affinity_enabled_override": false,
                "affinity_idle_ttl_seconds_override": 120,
                "affinity_failback_mode_override": "sticky",
                "affinity_failback_delay_seconds_override": 0,
                "models": { "gpt-5": { "redirect": null, "multiplier": "1" } }
            }
        ]
    }))
    .expect("update payload deserializes");

    let updated = store
        .update_provider(&created.id, update_body)
        .await
        .expect("provider updated");

    assert_eq!(
        updated.group_ids,
        vec!["g-beta".to_string(), "g-alpha".to_string()]
    );
    assert_eq!(updated.channels[0].api_key, "secret");
    assert_eq!(updated.channels[0].affinity_enabled_override, Some(false));
    assert_eq!(
        updated.channels[0].affinity_idle_ttl_seconds_override,
        Some(120)
    );
    assert_eq!(
        updated.channels[0].affinity_failback_mode_override,
        Some(AffinityFailbackMode::Sticky)
    );
    assert_eq!(
        updated.channels[0].affinity_failback_delay_seconds_override,
        Some(0)
    );

    // GR-I2: clearing the selection falls back to the default group instead
    // of leaving an empty (formerly "public") set.
    let cleared = store
        .update_provider(
            &created.id,
            serde_json::from_value(json!({
                "group_ids": []
            }))
            .expect("clear payload deserializes"),
        )
        .await
        .expect("provider group selection cleared");
    assert_eq!(cleared.group_ids, vec![default_group_id]);

    // GR-C3: unknown registry ids are rejected.
    let unknown_err = store
        .update_provider(
            &created.id,
            serde_json::from_value(json!({
                "group_ids": ["g-missing"]
            }))
            .expect("unknown payload deserializes"),
        )
        .await
        .expect_err("unknown group id rejected");
    assert!(unknown_err.contains("unknown group id"));
}

#[test]
fn dashboard_create_user_group_id_defaults_to_absent() {
    let body: CreateUserRequest = serde_json::from_value(json!({
        "username": "alice",
        "password": "password123",
        "role": "user"
    }))
    .expect("payload deserializes");

    // U3: absent group_id means "assign the default group" downstream.
    assert!(body.group_id.is_none());
}

#[test]
fn dashboard_create_api_key_defaults_to_inheriting_user_group() {
    let body: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "default key"
    }))
    .expect("payload deserializes");

    assert!(body.use_user_group);
    assert!(body.group_ids.is_empty());

    let explicit: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "explicit key",
        "use_user_group": false,
        "group_ids": ["g-2", "g-1"]
    }))
    .expect("payload deserializes");
    assert!(!explicit.use_user_group);
    assert_eq!(
        explicit.group_ids,
        vec!["g-2".to_string(), "g-1".to_string()]
    );
}

#[test]
fn dashboard_update_api_key_group_fields_are_partial() {
    let omitted: UpdateApiKeyRequest = serde_json::from_value(json!({
        "name": "renamed key"
    }))
    .expect("payload deserializes");
    assert!(omitted.use_user_group.is_none());
    assert!(omitted.group_ids.is_none());

    let present: UpdateApiKeyRequest = serde_json::from_value(json!({
        "use_user_group": false,
        "group_ids": ["g-2", "g-1"]
    }))
    .expect("payload deserializes");
    assert_eq!(present.use_user_group, Some(false));
    assert_eq!(
        present.group_ids,
        Some(vec!["g-2".to_string(), "g-1".to_string()])
    );
}

#[tokio::test]
async fn dashboard_user_group_id_round_trip_through_store_and_response() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");
    let default_group_id = store.default_group_id().await.expect("default exists");
    let team = store
        .create_group(crate::users::CreateGroupInput {
            name: "team".to_string(),
            description: "team routing".to_string(),
            user_selectable: true,
            sort_order: 1,
        })
        .await
        .expect("team group created");

    let create_body: CreateUserRequest = serde_json::from_value(json!({
        "username": "alice",
        "password": "password123",
        "role": "user",
        "group_id": team.id
    }))
    .expect("create payload deserializes");

    let created = store
        .create_user(
            &create_body.username,
            &create_body.password,
            UserRole::User,
            create_body.group_id.as_deref(),
        )
        .await
        .expect("user created");
    assert_eq!(created.group_id, team.id);

    // U3: an omitted group id resolves to the system default group.
    let defaulted = store
        .create_user("bob", "password123", UserRole::User, None)
        .await
        .expect("defaulted user created");
    assert_eq!(defaulted.group_id, default_group_id);

    // GR-C3: assignments must reference a registry row.
    let unknown_err = store
        .create_user("carol", "password123", UserRole::User, Some("g-missing"))
        .await
        .expect_err("unknown group id rejected");
    assert!(unknown_err.contains("unknown group id"));

    let update_body: UpdateUserRequest = serde_json::from_value(json!({
        "group_id": default_group_id
    }))
    .expect("update payload deserializes");
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
            update_body.group_id.as_deref(),
        )
        .await
        .expect("user updated");

    let fetched = store
        .get_user_by_id(&created.id)
        .await
        .expect("lookup succeeds")
        .expect("user exists");
    assert_eq!(fetched.group_id, default_group_id);

    let listed = store.list_users().await.expect("list succeeds");
    let listed_user = listed
        .into_iter()
        .find(|user| user.id == created.id)
        .expect("listed user exists");
    assert_eq!(listed_user.group_id, default_group_id);

    let response = serde_json::to_value(UserResponse::from(fetched)).expect("response serializes");
    assert_eq!(response.get("group_id"), Some(&json!(default_group_id)));
    assert_eq!(response.get("billing_plan"), Some(&json!(null)));
    assert!(response.get("today_calls").is_none());
}

#[test]
fn user_response_serializes_group_id() {
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
        group_id: "g-alpha".to_string(),
        billing_plan_id: None,
        next_grant_at: None,
    };

    let value = serde_json::to_value(UserResponse::from(user)).expect("response serializes");
    assert_eq!(value.get("group_id"), Some(&json!("g-alpha")));
    assert_eq!(value.get("billing_plan"), Some(&json!(null)));
    assert!(value.get("today_calls").is_none());
    assert!(value.get("today_cost_nano_usd").is_none());
}

#[tokio::test]
async fn dashboard_api_key_group_selection_round_trip_through_store_and_responses() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");
    let alpha = store
        .create_group(crate::users::CreateGroupInput {
            name: "alpha".to_string(),
            description: String::new(),
            user_selectable: true,
            sort_order: 1,
        })
        .await
        .expect("alpha group created");
    let beta = store
        .create_group(crate::users::CreateGroupInput {
            name: "beta".to_string(),
            description: String::new(),
            user_selectable: true,
            sort_order: 2,
        })
        .await
        .expect("beta group created");

    let user = store
        .create_user("alice", "password123", UserRole::User, None)
        .await
        .expect("user created");

    let create_body: CreateApiKeyRequest = serde_json::from_value(json!({
        "name": "dashboard key",
        "use_user_group": false,
        "group_ids": [format!(" {} ", beta.id), alpha.id, beta.id, ""]
    }))
    .expect("create payload deserializes");

    let (created, key) = store
        .create_api_key_extended(
            &user.id,
            CreateApiKeyInput {
                name: create_body.name,
                expires_in_days: create_body.expires_in_days,
                sub_account_enabled: create_body.sub_account_enabled,
                sub_account_balance_nano_usd: create_body.sub_account_balance_nano_usd,
                model_limits_enabled: create_body.model_limits_enabled,
                model_limits: create_body.model_limits,
                ip_whitelist: create_body.ip_whitelist,
                use_user_group: create_body.use_user_group,
                group_ids: create_body.group_ids,
                max_multiplier: create_body.max_multiplier,
                transforms: create_body.transforms,
                model_redirects: create_body.model_redirects,
                reasoning_envelope_enabled: create_body.reasoning_envelope_enabled,
                request_capture_mode: create_body.request_capture_mode,
            },
            false,
        )
        .await
        .expect("api key created");

    // Order is routing priority (TM-GRP-2): trim + dedupe, keep submitted order.
    assert!(!created.use_user_group);
    assert_eq!(created.group_ids, vec![beta.id.clone(), alpha.id.clone()]);

    let (nano, usd) = super::api_keys::nano_balance_fields(&created.sub_account_balance_nano)
        .expect("stored balance is valid");
    let created_value = serde_json::to_value(ApiKeyCreatedResponse {
        id: created.id.clone(),
        name: created.name.clone(),
        key,
        key_prefix: created.key_prefix.clone(),
        created_at: created.created_at.to_rfc3339(),
        expires_at: created.expires_at.map(|date| date.to_rfc3339()),
        sub_account_enabled: created.sub_account_enabled,
        sub_account_balance_nano_usd: nano,
        sub_account_balance_usd: usd,
        model_limits_enabled: created.model_limits_enabled,
        model_limits: created.model_limits.clone(),
        ip_whitelist: created.ip_whitelist.clone(),
        use_user_group: created.use_user_group,
        group_ids: created.group_ids.clone(),
        max_multiplier: created.max_multiplier,
        transforms: created.transforms.clone(),
        model_redirects: created.model_redirects.clone(),
        reasoning_envelope_enabled: created.reasoning_envelope_enabled,
        request_capture_mode: created.request_capture_mode,
    })
    .expect("created response serializes");
    assert_eq!(created_value.get("use_user_group"), Some(&json!(false)));
    assert_eq!(
        created_value.get("group_ids"),
        Some(&json!([beta.id.clone(), alpha.id.clone()]))
    );

    let update_body: UpdateApiKeyRequest = serde_json::from_value(json!({
        "group_ids": [format!(" {} ", beta.id), ""],
        "request_capture_mode": "capture-all"
    }))
    .expect("update payload deserializes");

    let updated = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                sub_account_enabled: None,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                use_user_group: update_body.use_user_group,
                group_ids: update_body.group_ids,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                reasoning_envelope_enabled: None,
                request_capture_mode: update_body.request_capture_mode,
                expires_at: None,
            },
            false,
        )
        .await
        .expect("api key updated");

    assert!(!updated.use_user_group);
    assert_eq!(updated.group_ids, vec![beta.id.clone()]);
    assert_eq!(
        updated.request_capture_mode,
        crate::users::RequestCaptureMode::CaptureAll
    );

    let fetched = store
        .get_api_key_by_id(&updated.id)
        .await
        .expect("lookup succeeds")
        .expect("api key exists");
    assert_eq!(fetched.group_ids, vec![beta.id.clone()]);

    let listed_key = store
        .list_user_api_keys(&user.id)
        .await
        .expect("list succeeds")
        .into_iter()
        .find(|api_key| api_key.id == updated.id)
        .expect("listed api key exists");
    assert_eq!(listed_key.group_ids, vec![beta.id.clone()]);

    // TM-GRP-4: switching back to inheritance clears the stored selection.
    let inherited = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                sub_account_enabled: None,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                use_user_group: Some(true),
                group_ids: None,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                reasoning_envelope_enabled: None,
                request_capture_mode: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect("api key reverts to inheritance");
    assert!(inherited.use_user_group);
    assert!(inherited.group_ids.is_empty());

    let (fnano, fusd) = super::api_keys::nano_balance_fields(&fetched.sub_account_balance_nano)
        .expect("stored balance is valid");
    let response_value = serde_json::to_value(ApiKeyResponse {
        id: fetched.id,
        name: fetched.name,
        key_prefix: fetched.key_prefix,
        key: fetched.key,
        created_at: fetched.created_at.to_rfc3339(),
        expires_at: fetched.expires_at.map(|date| date.to_rfc3339()),
        last_used_at: fetched.last_used_at.map(|date| date.to_rfc3339()),
        enabled: fetched.enabled,
        sub_account_enabled: fetched.sub_account_enabled,
        sub_account_balance_nano_usd: fnano,
        sub_account_balance_usd: fusd,
        model_limits_enabled: fetched.model_limits_enabled,
        model_limits: fetched.model_limits,
        ip_whitelist: fetched.ip_whitelist,
        use_user_group: fetched.use_user_group,
        group_ids: fetched.group_ids,
        max_multiplier: fetched.max_multiplier,
        transforms: fetched.transforms,
        model_redirects: fetched.model_redirects,
        reasoning_envelope_enabled: fetched.reasoning_envelope_enabled,
        request_capture_mode: fetched.request_capture_mode,
    })
    .expect("response serializes");
    assert_eq!(
        response_value.get("group_ids"),
        Some(&json!([beta.id.clone()]))
    );
    assert_eq!(
        response_value.get("request_capture_mode"),
        Some(&json!("capture-all"))
    );
}

#[tokio::test]
async fn admin_sub_account_adjustment_records_initial_credit_and_refund() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db.clone(), log_tx)
        .await
        .expect("store creates");
    let user = store
        .create_user("admin", "password123", UserRole::Admin, None)
        .await
        .expect("admin created");

    let (created, _) = store
        .create_api_key_extended(
            &user.id,
            CreateApiKeyInput {
                name: "funded key".to_string(),
                expires_in_days: None,
                sub_account_enabled: true,
                sub_account_balance_nano_usd: Some("200".to_string()),
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                use_user_group: true,
                group_ids: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: Vec::new(),
                reasoning_envelope_enabled: true,
                request_capture_mode: crate::users::RequestCaptureMode::Off,
            },
            true,
        )
        .await
        .expect("funded key created");
    assert_eq!(created.sub_account_balance_nano, "200");

    let updated = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                sub_account_enabled: None,
                sub_account_balance_nano_usd: Some("50".to_string()),
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                use_user_group: None,
                group_ids: None,
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                reasoning_envelope_enabled: None,
                request_capture_mode: None,
                expires_at: None,
            },
            true,
        )
        .await
        .expect("balance reduced");
    assert_eq!(updated.sub_account_balance_nano, "50");
    assert_eq!(
        store
            .get_user_balance(&user.id)
            .await
            .expect("balance lookup succeeds")
            .expect("balance exists")
            .balance_nano_usd,
        150
    );

    let rows = db
        .read()
        .query_all(db.stmt(
            "SELECT kind, delta_nano_usd FROM billing_ledger WHERE user_id = $1 ORDER BY created_at, id",
            vec![user.id.clone().into()],
        ))
        .await
        .expect("ledger query succeeds");
    let entries = rows
        .into_iter()
        .map(|row| {
            (
                row.try_get::<String>("", "kind").expect("kind"),
                row.try_get::<String>("", "delta_nano_usd").expect("delta"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entries,
        vec![
            (
                "admin_sub_account_adjustment".to_string(),
                "200".to_string()
            ),
            ("sub_account_refund".to_string(), "150".to_string()),
        ]
    );

    store
        .delete_api_key(&created.id)
        .await
        .expect("key deletion consolidates its balance");
    store
        .charge_sub_account_balance_nano(
            &created.id,
            &user.id,
            250,
            &json!({ "request_id": "request-admitted-before-delete" }),
        )
        .await
        .expect("admitted request settles to user after key deletion");
    assert_eq!(
        store
            .get_user_balance(&user.id)
            .await
            .expect("balance lookup succeeds")
            .expect("balance exists")
            .balance_nano_usd,
        -50
    );
}

#[tokio::test]
async fn dashboard_api_key_group_selection_enforces_registry_and_selectability() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let (log_tx, _) = tokio::sync::broadcast::channel(1);
    let store = UserStore::new(db, log_tx).await.expect("store creates");
    let hidden = store
        .create_group(crate::users::CreateGroupInput {
            name: "hidden".to_string(),
            description: String::new(),
            user_selectable: false,
            sort_order: 1,
        })
        .await
        .expect("hidden group created");

    let user = store
        .create_user("member", "password123", UserRole::User, None)
        .await
        .expect("user created");

    fn key_input(name: &str, use_user_group: bool, group_ids: Vec<String>) -> CreateApiKeyInput {
        CreateApiKeyInput {
            name: name.to_string(),
            expires_in_days: None,
            sub_account_enabled: false,
            sub_account_balance_nano_usd: None,
            model_limits_enabled: false,
            model_limits: Vec::new(),
            ip_whitelist: Vec::new(),
            use_user_group,
            group_ids,
            max_multiplier: None,
            transforms: Vec::new(),
            model_redirects: Vec::new(),
            reasoning_envelope_enabled: true,
            request_capture_mode: crate::users::RequestCaptureMode::Off,
        }
    }

    // TM-GRP-5: non-admins may not select a non-user_selectable group.
    let create_err = store
        .create_api_key_extended(
            &user.id,
            key_input("hidden key", false, vec![hidden.id.clone()]),
            false,
        )
        .await
        .expect_err("create should reject non-selectable group");
    assert!(create_err.contains("not selectable"));

    // TM-GRP-3: an explicit selection must be non-empty.
    let empty_err = store
        .create_api_key_extended(&user.id, key_input("empty key", false, Vec::new()), false)
        .await
        .expect_err("create should reject empty explicit selection");
    assert!(empty_err.contains("non-empty"));

    let (created, _) = store
        .create_api_key_extended(&user.id, key_input("baseline key", true, Vec::new()), false)
        .await
        .expect("baseline key created");

    let update_err = store
        .update_api_key(
            &created.id,
            UpdateApiKeyInput {
                name: None,
                enabled: None,
                sub_account_enabled: None,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                use_user_group: Some(false),
                group_ids: Some(vec![hidden.id.clone()]),
                max_multiplier: None,
                transforms: None,
                model_redirects: None,
                reasoning_envelope_enabled: None,
                request_capture_mode: None,
                expires_at: None,
            },
            false,
        )
        .await
        .expect_err("update should reject non-selectable group");
    assert!(update_err.contains("not selectable"));

    // Admin callers bypass the user_selectable restriction.
    let admin_key = store
        .create_api_key_extended(
            &user.id,
            key_input("admin key", false, vec![hidden.id.clone()]),
            true,
        )
        .await
        .expect("admin may select any registered group");
    assert_eq!(admin_key.0.group_ids, vec![hidden.id.clone()]);
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
        .create_user("redirect-user", "password123", UserRole::User, None)
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
                sub_account_enabled: create_body.sub_account_enabled,
                sub_account_balance_nano_usd: create_body.sub_account_balance_nano_usd,
                model_limits_enabled: create_body.model_limits_enabled,
                model_limits: create_body.model_limits,
                ip_whitelist: create_body.ip_whitelist,
                use_user_group: create_body.use_user_group,
                group_ids: create_body.group_ids,
                max_multiplier: create_body.max_multiplier,
                transforms: create_body.transforms,
                model_redirects: create_body.model_redirects,
                reasoning_envelope_enabled: create_body.reasoning_envelope_enabled,
                request_capture_mode: create_body.request_capture_mode,
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
                sub_account_enabled: None,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: None,
                model_limits: None,
                ip_whitelist: None,
                use_user_group: None,
                group_ids: None,
                max_multiplier: None,
                transforms: None,
                model_redirects: Some(vec![ModelRedirectRule {
                    pattern: ".*sonnet.*".to_string(),
                    replace: "gpt-5.4".to_string(),
                }]),
                reasoning_envelope_enabled: None,
                request_capture_mode: None,
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
                sub_account_enabled: false,
                sub_account_balance_nano_usd: None,
                model_limits_enabled: false,
                model_limits: Vec::new(),
                ip_whitelist: Vec::new(),
                use_user_group: true,
                group_ids: Vec::new(),
                max_multiplier: None,
                transforms: Vec::new(),
                model_redirects: vec![ModelRedirectRule {
                    pattern: "(".to_string(),
                    replace: "gpt-5.4".to_string(),
                }],
                reasoning_envelope_enabled: true,
                request_capture_mode: crate::users::RequestCaptureMode::Off,
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
        effective_provider_type: Some("responses".to_string()),
        request_kind: None,
        reasoning_effort: None,
        request_ip: None,
        tried_providers: None,
        session_affinity_value: Some("ses-1".to_string()),
        provider: RequestLogProvider {
            id: Some("provider-1".to_string()),
            name: Some("Provider".to_string()),
            multiplier: Some(crate::exact_decimal::Multiplier::ONE),
        },
        channel: RequestLogChannel {
            id: Some("channel-1".to_string()),
            name: Some("Channel".to_string()),
        },
        affinity: RequestLogAffinity {
            hit: Some(false),
            key_hash: Some("abc123".to_string()),
            target: Some("provider-1/channel-1".to_string()),
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
        has_capture: false,
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

    let request_log_columns: i64 = db
        .read()
        .query_one(db.stmt(
            "SELECT COUNT(*) AS column_count FROM pragma_table_info('request_logs')",
            vec![],
        ))
        .await
        .expect("count request-log columns")
        .expect("request-log column count exists")
        .try_get("", "column_count")
        .expect("request-log column count decodes");
    // 42 columns from the RL-S3b rebuild, plus session_affinity_value (RL-S4),
    // minus the five visible-TPS columns dropped per RL-S12.
    assert_eq!(request_log_columns, 38);

    let request_log_foreign_keys = db
        .read()
        .query_all(db.stmt(
            "SELECT id FROM pragma_foreign_key_list('request_logs')",
            vec![],
        ))
        .await
        .expect("list request-log foreign keys");
    assert!(request_log_foreign_keys.is_empty());

    let channel_columns = db
        .read()
        .query_all(db.stmt(
            "SELECT name FROM pragma_table_info('monoize_channels')",
            vec![],
        ))
        .await
        .expect("list channel columns")
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "name").ok())
        .collect::<std::collections::HashSet<_>>();
    for column in [
        "affinity_enabled_override",
        "affinity_idle_ttl_seconds_override",
        "affinity_failback_mode_override",
        "affinity_failback_delay_seconds_override",
    ] {
        assert!(channel_columns.contains(column), "missing column {column}");
    }
}

#[tokio::test]
async fn settings_store_round_trips_global_transforms_and_model_redirects() {
    let db = DbPool::connect("sqlite::memory:")
        .await
        .expect("db connects");
    {
        let write = db.write().await;
        Migrator::up(&*write, None).await.expect("migrates");
    }

    let store = SettingsStore::new(db).await.expect("store creates");
    let mut settings = store.get_all().await.expect("settings load");
    assert!(settings.global_transforms.is_empty());
    assert!(settings.global_model_redirects.is_empty());
    assert!(settings.codex_model_ids.is_empty());
    assert!(!settings.monoize_request_capture_enabled);
    assert_eq!(settings.monoize_request_capture_retention_days, 1);
    assert!(settings.monoize_affinity_enabled);
    assert_eq!(settings.monoize_affinity_idle_ttl_seconds, 1800);
    assert_eq!(
        settings.monoize_affinity_failback_mode,
        AffinityFailbackMode::Sticky
    );
    assert_eq!(settings.monoize_affinity_failback_delay_seconds, 300);

    settings.global_transforms = vec![TransformRuleConfig {
        transform: "remove_anthropic_billing_header".to_string(),
        enabled: true,
        models: Some(vec!["gpt-*".to_string()]),
        phase: Phase::Request,
        config: json!({}),
    }];
    settings.global_model_redirects = vec![ModelRedirectRule {
        pattern: "claude-.*".to_string(),
        replace: "gpt-5.6-sol".to_string(),
    }];
    settings.codex_model_ids = vec![
        " gpt-5.6-sol ".to_string(),
        "claude-opus-4.8".to_string(),
        "gpt-5.6-sol".to_string(),
        String::new(),
    ];
    settings.monoize_strip_cross_protocol_nested_extra = false;
    settings.monoize_request_capture_enabled = true;
    settings.monoize_request_capture_retention_days = 0;
    settings.monoize_affinity_enabled = false;
    settings.monoize_affinity_idle_ttl_seconds = 90;
    settings.monoize_affinity_failback_mode = AffinityFailbackMode::PreferHigherPriority;
    settings.monoize_affinity_failback_delay_seconds = 0;
    store.update_all(&settings).await.expect("settings update");

    let updated = store.get_all().await.expect("settings reload");
    assert_eq!(updated.global_transforms.len(), 1);
    assert_eq!(
        updated.global_transforms[0].transform,
        "prompt_strip_anthropic_billing_header"
    );
    assert_eq!(updated.global_transforms[0].phase, Phase::Request);
    assert_eq!(
        updated.global_model_redirects,
        vec![ModelRedirectRule {
            pattern: "claude-.*".to_string(),
            replace: "gpt-5.6-sol".to_string(),
        }]
    );
    assert_eq!(
        updated.codex_model_ids,
        vec!["gpt-5.6-sol", "claude-opus-4.8"]
    );
    assert!(!updated.monoize_strip_cross_protocol_nested_extra);
    assert!(updated.monoize_request_capture_enabled);
    assert_eq!(updated.monoize_request_capture_retention_days, 1);
    assert!(!updated.monoize_affinity_enabled);
    assert_eq!(updated.monoize_affinity_idle_ttl_seconds, 90);
    assert_eq!(
        updated.monoize_affinity_failback_mode,
        AffinityFailbackMode::PreferHigherPriority
    );
    assert_eq!(updated.monoize_affinity_failback_delay_seconds, 0);

    store
        .set("session_ttl_days", "21")
        .await
        .expect("session TTL updates");
    store
        .set("api_key_max_per_user", "37")
        .await
        .expect("API-key limit updates");
    store
        .set("registration_enabled", "false")
        .await
        .expect("registration flag updates");
    store
        .set("site_name", "Public Monoize")
        .await
        .expect("site name updates");
    store
        .set("site_description", "Public description")
        .await
        .expect("site description updates");
    store
        .set("api_base_url", "https://api.example.test")
        .await
        .expect("API URL updates");

    assert_eq!(store.get_session_ttl_days().await.unwrap(), 21);
    assert_eq!(store.get_api_key_max_per_user().await.unwrap(), 37);
    let public = store.get_public_settings().await.unwrap();
    assert!(!public.registration_enabled);
    assert_eq!(public.site_name, "Public Monoize");
    assert_eq!(public.site_description, "Public description");
    assert_eq!(public.api_base_url, "https://api.example.test");

    store
        .set("registration_enabled", "not-a-boolean")
        .await
        .expect("malformed registration flag persists for decode test");
    assert!(store.is_registration_enabled().await.is_err());
    assert!(store.get_public_settings().await.is_err());
    assert!(store.get_all().await.is_err());
}

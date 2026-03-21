use super::providers::{
    build_models_list_url, provider_has_billable_pricing, provider_pricing_model,
};
use crate::db::DbPool;
use crate::migration::Migrator;
use crate::monoize_routing::MonoizeModelEntry;
use crate::providers::ProviderStore;
use crate::users::{
    RequestLogApiKey, RequestLogBilling, RequestLogChannel, RequestLogError, RequestLogProvider,
    RequestLogRow, RequestLogTiming, RequestLogTokens, RequestLogUser,
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

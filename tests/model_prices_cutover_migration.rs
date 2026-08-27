//! Integration tests for `m20260901_000048_model_prices_cutover`
//! (`model-pricing.spec.md` MP-M3..MP-M6).

use monoize::migration::Migrator;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

async fn connect() -> DatabaseConnection {
    Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite")
}

/// Applies every migration except the cutover step.
async fn migrate_to_pre_cutover(db: &DatabaseConnection) {
    let cutover_index = Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20260901_000048_model_prices_cutover")
        .expect("cutover migration is registered") as u32;
    Migrator::up(db, Some(cutover_index))
        .await
        .expect("pre-cutover migrations apply");
}

fn cutover_and_later_migration_count() -> u32 {
    let migrations = Migrator::migrations();
    let cutover_index = migrations
        .iter()
        .position(|migration| migration.name() == "m20260901_000048_model_prices_cutover")
        .expect("cutover migration is registered");
    (migrations.len() - cutover_index) as u32
}

async fn insert_legacy_rule(
    db: &DatabaseConnection,
    id: &str,
    source: &str,
    model_pattern: Option<&str>,
    rate_kind: &str,
    usage_class: &str,
    unit_price_nano_usd: &str,
    priority: i32,
    enabled: i32,
    context_tier: Option<&str>,
    service_tier: Option<&str>,
    modality: Option<&str>,
) {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO billing_rate_records \
         (id, source, pricing_profile, model_pattern, provider_type, rate_kind, usage_class, \
          unit, unit_price_nano_usd, context_tier, service_tier, modality, cache_ttl, \
          match_json, priority, enabled, raw_json, updated_at) \
         VALUES (?1, ?2, 'default', ?3, NULL, ?4, ?5, 'token', ?6, ?7, ?8, ?9, NULL, '{}', \
                 ?10, ?11, '{}', '2026-01-01T00:00:00Z')",
        [
            id.into(),
            source.into(),
            model_pattern.into(),
            rate_kind.into(),
            usage_class.into(),
            unit_price_nano_usd.into(),
            context_tier.into(),
            service_tier.into(),
            modality.into(),
            priority.into(),
            enabled.into(),
        ],
    ))
    .await
    .expect("insert legacy rule");
}

async fn table_exists(db: &DatabaseConnection, table: &str) -> bool {
    db.query_one(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table.into()],
    ))
    .await
    .expect("query sqlite_master")
    .is_some()
}

async fn table_columns(db: &DatabaseConnection, table: &str) -> Vec<String> {
    db.query_all(Statement::from_string(
        DbBackend::Sqlite,
        format!("SELECT name FROM pragma_table_info('{table}')"),
    ))
    .await
    .expect("pragma table info")
    .into_iter()
    .filter_map(|row| row.try_get::<String>("", "name").ok())
    .collect()
}

#[tokio::test]
async fn cutover_converts_eligible_manual_token_rules_and_drops_legacy_schema() {
    let db = connect().await;
    migrate_to_pre_cutover(&db).await;

    // Eligible: manual, enabled, token, exact pattern, dimensionless tiers.
    insert_legacy_rule(
        &db,
        "m1",
        "manual",
        Some("gpt-4o"),
        "token",
        "input_uncached",
        "2500",
        0,
        1,
        None,
        None,
        None,
    )
    .await;
    insert_legacy_rule(
        &db,
        "m2",
        "manual",
        Some("gpt-4o"),
        "token",
        "output",
        "10000",
        0,
        1,
        Some("default"),
        Some("default"),
        None,
    )
    .await;
    insert_legacy_rule(
        &db,
        "m3",
        "manual",
        Some("gpt-4o"),
        "token",
        "input_cached",
        "1250",
        0,
        1,
        None,
        None,
        None,
    )
    .await;
    // Conflicting input rules: higher priority wins (MP-M3).
    insert_legacy_rule(
        &db,
        "m4",
        "manual",
        Some("gpt-4o"),
        "token",
        "input_uncached",
        "9999",
        5,
        1,
        None,
        None,
        None,
    )
    .await;
    // Discarded: glob pattern (MP-M4).
    insert_legacy_rule(
        &db,
        "g1",
        "manual",
        Some("claude-*"),
        "token",
        "input_uncached",
        "3000",
        0,
        1,
        None,
        None,
        None,
    )
    .await;
    // Discarded: meter rate kind.
    insert_legacy_rule(
        &db,
        "t1",
        "manual",
        Some("tool-model"),
        "meter",
        "web_search",
        "10000000",
        0,
        1,
        None,
        None,
        None,
    )
    .await;
    // Discarded: models_dev source (operators re-sync via §9).
    insert_legacy_rule(
        &db,
        "s1",
        "models_dev",
        Some("gemini-2.5-pro"),
        "token",
        "input_uncached",
        "1250",
        0,
        1,
        None,
        None,
        None,
    )
    .await;
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "INSERT INTO billing_rate_records \
         (id, source, pricing_profile, model_pattern, provider_type, rate_kind, usage_class, \
          unit, unit_price_nano_usd, match_json, priority, enabled, raw_json, updated_at) \
         VALUES ('p1', 'manual', 'default', 'provider-model', 'openai', 'token', \
                 'input_uncached', 'token', '1000', '{}', 0, 1, '{}', \
                 '2026-01-01T00:00:00Z')"
            .to_string(),
    ))
    .await
    .expect("insert provider-scoped legacy rule");
    // Discarded: disabled rule.
    insert_legacy_rule(
        &db,
        "d1",
        "manual",
        Some("disabled-model"),
        "token",
        "input_uncached",
        "1000",
        0,
        0,
        None,
        None,
        None,
    )
    .await;
    // Discarded: modality-scoped rule.
    insert_legacy_rule(
        &db,
        "mod1",
        "manual",
        Some("audio-model"),
        "token",
        "input_uncached",
        "1000",
        0,
        1,
        None,
        None,
        Some("audio"),
    )
    .await;
    // Suffixed pattern normalizes to the base pricing key.
    insert_legacy_rule(
        &db,
        "sfx1",
        "manual",
        Some("gpt-5-mini-high"),
        "token",
        "output",
        "3000",
        0,
        1,
        None,
        None,
        None,
    )
    .await;
    // A pre-existing model_prices row keeps its values (MP-M3).
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "INSERT INTO model_prices (model_id, billing_mode, input_usd_per_1m, source, \
         locked_fields, raw_json, enabled, updated_at) \
         VALUES ('kept-model', 'per_token', '42', 'manual', '[]', '{}', 1, \
                 '2026-01-01T00:00:00Z')"
            .to_string(),
    ))
    .await
    .expect("insert existing model price");
    insert_legacy_rule(
        &db,
        "k1",
        "manual",
        Some("kept-model"),
        "token",
        "input_uncached",
        "1000",
        0,
        1,
        None,
        None,
        None,
    )
    .await;

    Migrator::up(&db, None).await.expect("cutover applies");

    // MP-M3: converted row with winner selection and locked fields.
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT billing_mode, input_usd_per_1m, output_usd_per_1m, \
             cache_read_usd_per_1m, source, locked_fields \
             FROM model_prices WHERE model_id = 'gpt-4o'"
                .to_string(),
        ))
        .await
        .expect("query converted row")
        .expect("converted row exists");
    assert_eq!(
        row.try_get::<String>("", "billing_mode").unwrap(),
        "per_token"
    );
    // 9999 nano/token at priority 5 beats 2500 at priority 0: 9.999 USD/1M.
    assert_eq!(
        row.try_get::<String>("", "input_usd_per_1m").unwrap(),
        "9.999"
    );
    assert_eq!(
        row.try_get::<String>("", "output_usd_per_1m").unwrap(),
        "10"
    );
    assert_eq!(
        row.try_get::<String>("", "cache_read_usd_per_1m").unwrap(),
        "1.25"
    );
    assert_eq!(row.try_get::<String>("", "source").unwrap(), "manual");
    let locked: Vec<String> =
        serde_json::from_str(&row.try_get::<String>("", "locked_fields").unwrap()).unwrap();
    assert!(locked.contains(&"input_usd_per_1m".to_string()));
    assert!(locked.contains(&"output_usd_per_1m".to_string()));
    assert!(locked.contains(&"cache_read_usd_per_1m".to_string()));

    // Suffixed pattern converts under its normalized key.
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT output_usd_per_1m FROM model_prices WHERE model_id = 'gpt-5-mini'".to_string(),
        ))
        .await
        .expect("query suffixed row")
        .expect("suffixed row converts to base key");
    assert_eq!(row.try_get::<String>("", "output_usd_per_1m").unwrap(), "3");

    // The existing row keeps its stored values.
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT input_usd_per_1m FROM model_prices WHERE model_id = 'kept-model'".to_string(),
        ))
        .await
        .expect("query kept row")
        .expect("kept row exists");
    assert_eq!(row.try_get::<String>("", "input_usd_per_1m").unwrap(), "42");

    // MP-M4: discarded rules produce no rows.
    for absent in [
        "claude-*",
        "tool-model",
        "gemini-2.5-pro",
        "provider-model",
        "disabled-model",
        "audio-model",
    ] {
        let row = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT model_id FROM model_prices WHERE model_id = ?1",
                [absent.into()],
            ))
            .await
            .expect("query discarded");
        assert!(row.is_none(), "rule for {absent} must be discarded");
    }

    // MP-M5: legacy schema is gone.
    assert!(!table_exists(&db, "billing_rate_records").await);
    let channel_columns = table_columns(&db, "monoize_channels").await;
    assert!(!channel_columns.contains(&"allow_missing_usage".to_string()));
    assert!(!channel_columns.contains(&"allow_unpriced_server_tools".to_string()));
    let metadata_columns = table_columns(&db, "model_metadata_records").await;
    for dropped in [
        "input_cost_per_token_nano",
        "output_cost_per_token_nano",
        "cache_read_input_cost_per_token_nano",
        "cache_creation_input_cost_per_token_nano",
        "output_cost_per_reasoning_token_nano",
    ] {
        assert!(
            !metadata_columns.contains(&dropped.to_string()),
            "column {dropped} must be dropped"
        );
    }
    let setting = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT key FROM system_settings WHERE key = 'pricing_profile_model_patterns'"
                .to_string(),
        ))
        .await
        .expect("query settings");
    assert!(setting.is_none(), "legacy pattern setting must be deleted");
}

#[tokio::test]
async fn cutover_down_recreates_dropped_schema_empty() {
    let db = connect().await;
    Migrator::up(&db, None).await.expect("full migration");

    Migrator::down(&db, Some(cutover_and_later_migration_count()))
        .await
        .expect("cutover and later migrations down");

    assert!(table_exists(&db, "billing_rate_records").await);
    let count = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM billing_rate_records".to_string(),
        ))
        .await
        .expect("count rows")
        .expect("count row");
    assert_eq!(count.try_get::<i32>("", "n").unwrap(), 0);
    let channel_columns = table_columns(&db, "monoize_channels").await;
    assert!(channel_columns.contains(&"allow_missing_usage".to_string()));
    assert!(channel_columns.contains(&"allow_unpriced_server_tools".to_string()));
    let metadata_columns = table_columns(&db, "model_metadata_records").await;
    assert!(metadata_columns.contains(&"input_cost_per_token_nano".to_string()));

    // Re-applying the cutover after down() converts nothing and drops again.
    Migrator::up(&db, None).await.expect("cutover re-applies");
    assert!(!table_exists(&db, "billing_rate_records").await);
}

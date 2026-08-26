use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;
use std::collections::{BTreeMap, HashSet};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Debug)]
struct LegacyRate {
    id: String,
    model_pattern: String,
    usage_class: String,
    unit_price_nano_usd: String,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        if let Err(error) = migrate_up(&tx, backend).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        if !matches!(backend, DbBackend::Sqlite | DbBackend::Postgres) {
            return Ok(());
        }
        let tx = manager.get_connection().begin().await?;
        if let Err(error) = migrate_down(&tx, backend).await {
            let _ = tx.rollback().await;
            return Err(error);
        }
        tx.commit().await
    }
}

fn price_column(usage_class: &str) -> Option<&'static str> {
    match usage_class {
        "input_uncached" => Some("input_usd_per_1m"),
        "output" => Some("output_usd_per_1m"),
        "cache_read" | "input_cached" => Some("cache_read_usd_per_1m"),
        "cache_write_5m" => Some("cache_write_usd_per_1m"),
        "cache_write_1h" => Some("cache_write_1h_usd_per_1m"),
        "reasoning_output" => Some("reasoning_usd_per_1m"),
        _ => None,
    }
}

fn nano_per_token_to_usd_per_1m(raw: &str) -> Result<String, DbErr> {
    let value = raw
        .parse::<i128>()
        .map_err(|_| DbErr::Migration(format!("invalid legacy unit price `{raw}`")))?;
    if value < 0 {
        return Err(DbErr::Migration(format!(
            "negative legacy unit price `{raw}` cannot migrate"
        )));
    }
    let whole = value / 1000;
    let remainder = value % 1000;
    if remainder == 0 {
        return Ok(whole.to_string());
    }
    let fraction = format!("{remainder:03}").trim_end_matches('0').to_string();
    Ok(format!("{whole}.{fraction}"))
}

async fn migrate_legacy_manual_rates(
    tx: &DatabaseTransaction,
    backend: DbBackend,
) -> Result<(), DbErr> {
    let rows = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT id, model_pattern, usage_class, unit_price_nano_usd \
             FROM billing_rate_records \
             WHERE source = 'manual' AND enabled = 1 AND rate_kind = 'token' \
               AND model_pattern IS NOT NULL \
               AND model_pattern NOT LIKE '%*%' AND model_pattern NOT LIKE '%?%' \
               AND provider_type IS NULL AND modality IS NULL \
               AND (context_tier IS NULL OR context_tier = 'default') \
               AND (service_tier IS NULL OR service_tier = 'default') \
             ORDER BY priority DESC, id ASC"
                .to_string(),
        ))
        .await?;
    let legacy = rows
        .iter()
        .map(|row| {
            Ok(LegacyRate {
                id: row.try_get("", "id")?,
                model_pattern: row.try_get("", "model_pattern")?,
                usage_class: row.try_get("", "usage_class")?,
                unit_price_nano_usd: row.try_get("", "unit_price_nano_usd")?,
            })
        })
        .collect::<Result<Vec<_>, DbErr>>()?;
    let existing = tx
        .query_all(Statement::from_string(
            backend,
            "SELECT model_id FROM model_prices".to_string(),
        ))
        .await?
        .iter()
        .map(|row| row.try_get::<String>("", "model_id"))
        .collect::<Result<HashSet<_>, _>>()?;
    let mut converted: BTreeMap<String, BTreeMap<&'static str, String>> = BTreeMap::new();
    let mut selected: HashSet<(String, &'static str)> = HashSet::new();
    for rate in legacy {
        let Some(column) = price_column(&rate.usage_class) else {
            continue;
        };
        let model_id = crate::model_registry_store::normalize_model_id(&rate.model_pattern, None);
        if model_id.is_empty() || existing.contains(&model_id) {
            continue;
        }
        if !selected.insert((model_id.clone(), column)) {
            continue;
        }
        let _legacy_id = rate.id;
        converted.entry(model_id).or_default().insert(
            column,
            nano_per_token_to_usd_per_1m(&rate.unit_price_nano_usd)?,
        );
    }
    let now = chrono::Utc::now().to_rfc3339();
    for (model_id, columns) in converted {
        let locked_fields = serde_json::to_string(&columns.keys().copied().collect::<Vec<_>>())
            .map_err(|error| DbErr::Migration(error.to_string()))?;
        let value = |column: &str| columns.get(column).cloned();
        tx.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO model_prices (model_id, billing_mode, input_usd_per_1m, \
             output_usd_per_1m, cache_read_usd_per_1m, cache_write_usd_per_1m, \
             cache_write_1h_usd_per_1m, reasoning_usd_per_1m, per_request_usd, billing_expr, \
             source, locked_fields, raw_json, enabled, updated_at) \
             VALUES ($1, 'per_token', $2, $3, $4, $5, $6, $7, NULL, NULL, \
             'manual', $8, '{}', 1, $9) ON CONFLICT(model_id) DO NOTHING",
            vec![
                model_id.into(),
                value("input_usd_per_1m").into(),
                value("output_usd_per_1m").into(),
                value("cache_read_usd_per_1m").into(),
                value("cache_write_usd_per_1m").into(),
                value("cache_write_1h_usd_per_1m").into(),
                value("reasoning_usd_per_1m").into(),
                locked_fields.into(),
                now.clone().into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

async fn migrate_up(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    migrate_legacy_manual_rates(tx, backend).await?;
    let tool_prices = serde_json::json!({
        "web_search": "10",
        "x_search": "5",
        "file_search_tool_call": "2.5",
        "code_execution": "5",
        "code_interpreter_duration": { "usd": "0.0015", "per": "minute", "minimum_units": 5 },
        "code_execution_duration": { "usd": "0.000833333", "per": "minute", "minimum_units": 5 },
        "code_interpreter_session": { "usd": "0.03", "per": "session" }
    });
    tx.execute(Statement::from_sql_and_values(
        backend,
        "INSERT INTO system_settings (key, value, updated_at) VALUES \
         ('tool_prices', $1, $2) ON CONFLICT(key) DO NOTHING",
        vec![
            tool_prices.to_string().into(),
            chrono::Utc::now().to_rfc3339().into(),
        ],
    ))
    .await?;
    tx.execute(Statement::from_string(
        backend,
        "DELETE FROM system_settings WHERE key = 'pricing_profile_model_patterns'".to_string(),
    ))
    .await?;
    for sql in [
        "DROP TABLE billing_rate_records",
        "ALTER TABLE monoize_channels DROP COLUMN allow_missing_usage",
        "ALTER TABLE monoize_channels DROP COLUMN allow_unpriced_server_tools",
        "ALTER TABLE model_metadata_records DROP COLUMN input_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN output_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN cache_read_input_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN cache_creation_input_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN output_cost_per_reasoning_token_nano",
    ] {
        tx.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
    }
    Ok(())
}

async fn migrate_down(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    for sql in [
        "ALTER TABLE monoize_channels ADD COLUMN allow_missing_usage INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE monoize_channels ADD COLUMN allow_unpriced_server_tools INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE model_metadata_records ADD COLUMN input_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN output_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN cache_read_input_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN cache_creation_input_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN output_cost_per_reasoning_token_nano TEXT NULL",
        "CREATE TABLE billing_rate_records (id TEXT NOT NULL PRIMARY KEY, source TEXT NOT NULL, \
         pricing_profile TEXT NOT NULL, model_pattern TEXT NULL, provider_type TEXT NULL, \
         rate_kind TEXT NOT NULL, usage_class TEXT NOT NULL, unit TEXT NOT NULL, \
         unit_price_nano_usd TEXT NOT NULL, context_tier TEXT NULL, service_tier TEXT NULL, \
         modality TEXT NULL, cache_ttl TEXT NULL, match_json TEXT NOT NULL DEFAULT '{}', \
         priority INTEGER NOT NULL DEFAULT 0, enabled INTEGER NOT NULL DEFAULT 1, \
         raw_json TEXT NOT NULL DEFAULT '{}', updated_at TEXT NOT NULL)",
        "CREATE INDEX idx_billing_rate_records_lookup ON billing_rate_records \
         (pricing_profile, rate_kind, usage_class)",
    ] {
        tx.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
    }
    Ok(())
}

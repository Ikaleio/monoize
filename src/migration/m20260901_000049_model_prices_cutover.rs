use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use sea_orm_migration::prelude::*;
use std::collections::{BTreeMap, HashMap};

#[derive(DeriveMigrationName)]
pub struct Migration;

/// `model-pricing.spec.md` §12.2: destructive cutover step. Converts eligible
/// manual token rules into `model_prices` rows (MP-M3), discards every other
/// legacy rule (MP-M4), then drops `billing_rate_records`, the two Channel
/// free-usage columns, the `pricing_profile_model_patterns` setting, and the
/// `model_metadata_records` price columns (MP-M5). No compatibility alias
/// remains. `down()` recreates the dropped schema empty (MP-M6).
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

/// MP-M3 column mapping from legacy `usage_class` to `model_prices` columns.
fn price_column_for_usage_class(usage_class: &str) -> Option<&'static str> {
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

/// MP-M3 price conversion: `usd_per_1m = unit_price_nano_usd / 1000` exact.
/// A nano-USD-per-token integer maps losslessly to at most 9 fractional
/// digits, but 1000 nano per token is exactly 1 USD per 1M so the scale stays
/// at most 3 after division by 1000 for integer inputs.
fn nano_per_token_to_usd_per_1m(raw: &str) -> Option<String> {
    let value = rust_decimal::Decimal::from_str_exact(raw.trim()).ok()?;
    if value.is_sign_negative() {
        return None;
    }
    let usd = value.checked_div(rust_decimal::Decimal::from(1000u32))?;
    Some(
        usd.round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
            .normalize()
            .to_string(),
    )
}

/// The pricing-key normalization used at runtime: strip one configured or
/// built-in reasoning-effort suffix. Mirrors
/// `settings::normalize_pricing_model_key` with the map read from
/// `system_settings` inside this transaction.
fn normalize_model_key(model_id: &str, suffixes: &[String]) -> String {
    let trimmed = model_id.trim();
    for suffix in suffixes {
        if let Some(base) = trimmed.strip_suffix(suffix.as_str()) {
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    trimmed.to_string()
}

const BUILTIN_SUFFIXES: &[&str] = &[
    "-none", "-minimum", "-low", "-medium", "-high", "-xhigh", "-max",
];

struct LegacyRule {
    id: String,
    priority: i32,
    usd_per_1m: String,
}

async fn migrate_up(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    // Resolve the reasoning-suffix set the runtime uses for pricing keys.
    let suffix_row = tx
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT value FROM system_settings WHERE key = 'reasoning_suffix_map'",
            [],
        ))
        .await?;
    let mut suffixes: Vec<String> = Vec::new();
    if let Some(row) = suffix_row {
        let raw: String = row.try_get("", "value").unwrap_or_default();
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&raw) {
            suffixes.extend(map.into_keys());
        }
    }
    suffixes.extend(BUILTIN_SUFFIXES.iter().map(|s| String::from(*s)));
    // Longest-first so compound suffixes strip before their tails.
    suffixes.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    suffixes.dedup();

    // MP-M3: eligible manual token rules. Glob filtering happens in Rust so
    // SQLite and PostgreSQL share one query.
    let rows = tx
        .query_all(Statement::from_sql_and_values(
            backend,
            "SELECT id, model_pattern, usage_class, unit_price_nano_usd, priority \
             FROM billing_rate_records \
             WHERE source = 'manual' AND enabled = 1 AND rate_kind = 'token' \
               AND model_pattern IS NOT NULL \
               AND provider_type IS NULL \
               AND modality IS NULL \
               AND (context_tier IS NULL OR context_tier = 'default') \
               AND (service_tier IS NULL OR service_tier = 'default')",
            [],
        ))
        .await?;

    // model -> column -> winning legacy rule (higher priority, then lower id).
    let mut converted: BTreeMap<String, BTreeMap<&'static str, LegacyRule>> = BTreeMap::new();
    for row in &rows {
        let pattern: String = row
            .try_get("", "model_pattern")
            .map_err(|e| DbErr::Custom(e.to_string()))?;
        if pattern.contains('*') || pattern.contains('?') || pattern.trim().is_empty() {
            continue;
        }
        let usage_class: String = row
            .try_get("", "usage_class")
            .map_err(|e| DbErr::Custom(e.to_string()))?;
        let Some(column) = price_column_for_usage_class(&usage_class) else {
            continue;
        };
        let price_raw: String = row
            .try_get("", "unit_price_nano_usd")
            .map_err(|e| DbErr::Custom(e.to_string()))?;
        let Some(usd_per_1m) = nano_per_token_to_usd_per_1m(&price_raw) else {
            continue;
        };
        let rule = LegacyRule {
            id: row
                .try_get("", "id")
                .map_err(|e| DbErr::Custom(e.to_string()))?,
            priority: row
                .try_get("", "priority")
                .map_err(|e| DbErr::Custom(e.to_string()))?,
            usd_per_1m,
        };
        let model_key = normalize_model_key(&pattern, &suffixes);
        let slot = converted.entry(model_key).or_default();
        // MP-M3 tie-break: higher priority wins, then lower id.
        let keep_existing = slot.get(column).is_some_and(|existing| {
            existing.priority > rule.priority
                || (existing.priority == rule.priority && existing.id <= rule.id)
        });
        if !keep_existing {
            slot.insert(column, rule);
        }
    }

    // An existing model_prices row keeps its values; the legacy rule is
    // discarded (MP-M3).
    let existing_rows = tx
        .query_all(Statement::from_sql_and_values(
            backend,
            "SELECT model_id FROM model_prices",
            [],
        ))
        .await?;
    let existing_ids: std::collections::HashSet<String> = existing_rows
        .iter()
        .filter_map(|row| row.try_get("", "model_id").ok())
        .collect();

    let now = chrono::Utc::now().to_rfc3339();
    for (model_id, columns) in converted {
        if existing_ids.contains(&model_id) {
            continue;
        }
        let mut prices: HashMap<&'static str, String> = HashMap::new();
        let mut locked: Vec<&'static str> = Vec::new();
        for (column, rule) in columns {
            prices.insert(column, rule.usd_per_1m);
            locked.push(column);
        }
        let locked_json = serde_json::to_string(&locked)
            .map_err(|e| DbErr::Custom(format!("locked_fields serialize: {e}")))?;
        tx.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO model_prices (model_id, billing_mode, input_usd_per_1m, \
             output_usd_per_1m, cache_read_usd_per_1m, cache_write_usd_per_1m, \
             cache_write_1h_usd_per_1m, reasoning_usd_per_1m, per_request_usd, billing_expr, \
             source, locked_fields, raw_json, enabled, updated_at) \
             VALUES ($1, 'per_token', $2, $3, $4, $5, $6, $7, NULL, NULL, 'manual', $8, '{}', 1, $9)",
            [
                model_id.into(),
                prices.remove("input_usd_per_1m").into(),
                prices.remove("output_usd_per_1m").into(),
                prices.remove("cache_read_usd_per_1m").into(),
                prices.remove("cache_write_usd_per_1m").into(),
                prices.remove("cache_write_1h_usd_per_1m").into(),
                prices.remove("reasoning_usd_per_1m").into(),
                locked_json.into(),
                now.clone().into(),
            ],
        ))
        .await?;
    }

    // MP-M5 destructive steps.
    let drop_statements: &[&str] = &[
        "DROP TABLE billing_rate_records",
        "ALTER TABLE monoize_channels DROP COLUMN allow_missing_usage",
        "ALTER TABLE monoize_channels DROP COLUMN allow_unpriced_server_tools",
        "DELETE FROM system_settings WHERE key = 'pricing_profile_model_patterns'",
        "ALTER TABLE model_metadata_records DROP COLUMN input_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN output_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN cache_read_input_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN cache_creation_input_cost_per_token_nano",
        "ALTER TABLE model_metadata_records DROP COLUMN output_cost_per_reasoning_token_nano",
    ];
    for sql in drop_statements {
        tx.execute(Statement::from_string(backend, String::from(*sql)))
            .await?;
    }
    Ok(())
}

async fn migrate_down(tx: &DatabaseTransaction, backend: DbBackend) -> Result<(), DbErr> {
    // MP-M6: recreate the dropped schema empty; converted data stays in
    // model_prices and discarded rules are not reconstructible.
    for sql in [
        "CREATE TABLE billing_rate_records (\
         id TEXT NOT NULL PRIMARY KEY, \
         source TEXT NOT NULL, \
         pricing_profile TEXT NOT NULL, \
         model_pattern TEXT NULL, \
         provider_type TEXT NULL, \
         rate_kind TEXT NOT NULL, \
         usage_class TEXT NOT NULL, \
         unit TEXT NOT NULL, \
         unit_price_nano_usd TEXT NOT NULL, \
         context_tier TEXT NULL, \
         service_tier TEXT NULL, \
         modality TEXT NULL, \
         cache_ttl TEXT NULL, \
         match_json TEXT NOT NULL DEFAULT '{}', \
         priority INTEGER NOT NULL DEFAULT 0, \
         enabled INTEGER NOT NULL DEFAULT 1, \
         raw_json TEXT NOT NULL DEFAULT '{}', \
         updated_at TEXT NOT NULL)",
        "CREATE INDEX idx_billing_rate_records_lookup ON billing_rate_records \
         (pricing_profile, rate_kind, usage_class)",
        "ALTER TABLE monoize_channels ADD COLUMN allow_missing_usage INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE monoize_channels ADD COLUMN allow_unpriced_server_tools INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE model_metadata_records ADD COLUMN input_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN output_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN cache_read_input_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN cache_creation_input_cost_per_token_nano TEXT NULL",
        "ALTER TABLE model_metadata_records ADD COLUMN output_cost_per_reasoning_token_nano TEXT NULL",
    ] {
        tx.execute(Statement::from_string(backend, sql.to_string()))
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{nano_per_token_to_usd_per_1m, normalize_model_key, price_column_for_usage_class};

    #[test]
    fn nano_per_token_division_is_exact() {
        assert_eq!(nano_per_token_to_usd_per_1m("1000"), Some("1".to_string()));
        assert_eq!(
            nano_per_token_to_usd_per_1m("2500"),
            Some("2.5".to_string())
        );
        assert_eq!(nano_per_token_to_usd_per_1m("1"), Some("0.001".to_string()));
        assert_eq!(nano_per_token_to_usd_per_1m("0"), Some("0".to_string()));
        assert_eq!(nano_per_token_to_usd_per_1m("-5"), None);
    }

    #[test]
    fn usage_class_mapping_matches_mp_m3() {
        assert_eq!(
            price_column_for_usage_class("input_uncached"),
            Some("input_usd_per_1m")
        );
        assert_eq!(
            price_column_for_usage_class("input_cached"),
            Some("cache_read_usd_per_1m")
        );
        assert_eq!(
            price_column_for_usage_class("cache_write_1h"),
            Some("cache_write_1h_usd_per_1m")
        );
        assert_eq!(price_column_for_usage_class("web_search"), None);
    }

    #[test]
    fn model_key_strips_builtin_reasoning_suffix() {
        let suffixes = vec!["-high".to_string(), "-thinking".to_string()];
        assert_eq!(
            normalize_model_key("gpt-5-mini-high", &suffixes),
            "gpt-5-mini"
        );
        assert_eq!(
            normalize_model_key("claude-x-thinking", &suffixes),
            "claude-x"
        );
        assert_eq!(normalize_model_key("plain-model", &suffixes), "plain-model");
    }
}

//! `model_prices` and `price_sync_runs` persistence (`model-pricing.spec.md` §2, §10).

use crate::db::DbPool;
use crate::settings::validate_usd_decimal;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, QueryResult, Value as SeaValue};
use serde::{Deserialize, Serialize};

/// One `model_prices` row (MP-D1).
#[derive(Debug, Clone, Serialize)]
pub struct ModelPriceRecord {
    pub model_id: String,
    pub billing_mode: String,
    pub input_usd_per_1m: Option<String>,
    pub output_usd_per_1m: Option<String>,
    pub cache_read_usd_per_1m: Option<String>,
    pub cache_write_usd_per_1m: Option<String>,
    pub cache_write_1h_usd_per_1m: Option<String>,
    pub reasoning_usd_per_1m: Option<String>,
    pub per_request_usd: Option<String>,
    pub billing_expr: Option<serde_json::Value>,
    pub source: String,
    pub locked_fields: Vec<String>,
    pub raw_json: serde_json::Value,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}

impl ModelPriceRecord {
    /// MP-R3: row completeness by billing mode. Incomplete rows behave like
    /// missing rows (MP-R4).
    pub fn is_complete(&self) -> bool {
        match self.billing_mode.as_str() {
            "per_token" => self.input_usd_per_1m.is_some(),
            "per_request" => self.per_request_usd.is_some(),
            "tiered_expr" => self
                .billing_expr
                .as_ref()
                .is_some_and(|expr| validate_billing_expr(expr).is_ok()),
            _ => false,
        }
    }
}

/// MP-A2 upsert body: omitted = keep stored, explicit null = clear.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpsertModelPriceInput {
    pub billing_mode: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub input_usd_per_1m: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub output_usd_per_1m: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub cache_read_usd_per_1m: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub cache_write_usd_per_1m: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub cache_write_1h_usd_per_1m: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub reasoning_usd_per_1m: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub per_request_usd: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub billing_expr: Option<Option<serde_json::Value>>,
    pub locked_fields: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

fn deserialize_double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

/// One `price_sync_runs` row (MP-D7).
#[derive(Debug, Clone, Serialize)]
pub struct PriceSyncRun {
    pub id: String,
    pub source: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub inserted: i32,
    pub updated: i32,
    pub skipped: i32,
    pub deleted: i32,
    pub error: Option<String>,
    pub detail_json: serde_json::Value,
}

const BILLING_MODES: &[&str] = &["per_token", "per_request", "tiered_expr"];

/// MP-D4 whitelist of lockable field names.
pub const LOCKABLE_FIELDS: &[&str] = &[
    "billing_mode",
    "input_usd_per_1m",
    "output_usd_per_1m",
    "cache_read_usd_per_1m",
    "cache_write_usd_per_1m",
    "cache_write_1h_usd_per_1m",
    "reasoning_usd_per_1m",
    "per_request_usd",
    "billing_expr",
    "enabled",
];

/// Price fields a `billing_expr` tier may set (MP-C6).
const TIER_PRICE_FIELDS: &[&str] = &[
    "input_usd_per_1m",
    "output_usd_per_1m",
    "cache_read_usd_per_1m",
    "cache_write_usd_per_1m",
    "cache_write_1h_usd_per_1m",
    "reasoning_usd_per_1m",
];

/// MP-C6/MP-C7 write-time validation for `billing_expr`.
pub fn validate_billing_expr(expr: &serde_json::Value) -> Result<(), String> {
    let object = expr
        .as_object()
        .ok_or_else(|| "billing_expr must be a JSON object".to_string())?;
    for key in object.keys() {
        if key != "tiers" {
            return Err(format!("billing_expr: unknown field `{key}`"));
        }
    }
    let tiers = object
        .get("tiers")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "billing_expr.tiers must be an array".to_string())?;
    if tiers.is_empty() || tiers.len() > 8 {
        return Err("billing_expr.tiers must contain 1 to 8 tiers".to_string());
    }
    let mut previous_bound: Option<u64> = None;
    for (index, tier) in tiers.iter().enumerate() {
        let tier = tier
            .as_object()
            .ok_or_else(|| format!("billing_expr.tiers[{index}] must be an object"))?;
        let is_last = index == tiers.len() - 1;
        match tier.get("when_input_tokens_lte") {
            Some(bound) if !is_last => {
                let bound = bound
                    .as_u64()
                    .filter(|value| *value >= 1)
                    .ok_or_else(|| {
                        format!(
                            "billing_expr.tiers[{index}].when_input_tokens_lte must be an integer >= 1"
                        )
                    })?;
                if previous_bound.is_some_and(|previous| bound <= previous) {
                    return Err(
                        "billing_expr tier bounds must be strictly increasing".to_string()
                    );
                }
                previous_bound = Some(bound);
            }
            Some(serde_json::Value::Null) | None if is_last => {}
            Some(_) => {
                return Err("the last billing_expr tier must omit when_input_tokens_lte"
                    .to_string());
            }
            None => {
                return Err(format!(
                    "billing_expr.tiers[{index}] must set when_input_tokens_lte"
                ));
            }
        }
        let mut has_input_price = false;
        for (key, value) in tier {
            if key == "when_input_tokens_lte" {
                continue;
            }
            if !TIER_PRICE_FIELDS.contains(&key.as_str()) {
                return Err(format!("billing_expr.tiers[{index}]: unknown field `{key}`"));
            }
            match value {
                serde_json::Value::Null => {}
                serde_json::Value::String(raw) => {
                    validate_usd_decimal(raw)
                        .map_err(|message| format!("billing_expr.tiers[{index}].{key}: {message}"))?;
                    if key == "input_usd_per_1m" {
                        has_input_price = true;
                    }
                }
                _ => {
                    return Err(format!(
                        "billing_expr.tiers[{index}].{key} must be a decimal string or null"
                    ));
                }
            }
        }
        if !has_input_price {
            return Err(format!(
                "billing_expr.tiers[{index}] must set a non-null input_usd_per_1m"
            ));
        }
    }
    Ok(())
}

fn validate_locked_fields(fields: &[String]) -> Result<(), String> {
    for field in fields {
        if !LOCKABLE_FIELDS.contains(&field.as_str()) {
            return Err(format!("locked_fields: unknown field `{field}`"));
        }
    }
    Ok(())
}

fn validate_price_option(column: &str, value: &Option<String>) -> Result<(), String> {
    if let Some(raw) = value {
        validate_usd_decimal(raw).map_err(|message| format!("{column}: {message}"))?;
    }
    Ok(())
}

fn parse_time(raw: &str, column: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| format!("invalid {column} RFC3339: {error}"))
}

const MODEL_PRICE_COLUMNS: &str = "model_id, billing_mode, input_usd_per_1m, \
     output_usd_per_1m, cache_read_usd_per_1m, cache_write_usd_per_1m, \
     cache_write_1h_usd_per_1m, reasoning_usd_per_1m, per_request_usd, billing_expr, \
     source, locked_fields, raw_json, enabled, updated_at";

fn row_to_record(row: &QueryResult) -> Result<ModelPriceRecord, String> {
    let model_id: String = row.try_get("", "model_id").map_err(|e| e.to_string())?;
    let billing_expr_raw: Option<String> =
        row.try_get("", "billing_expr").map_err(|e| e.to_string())?;
    let billing_expr = billing_expr_raw
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|error| format!("model_prices[{model_id}] invalid billing_expr: {error}"))
        })
        .transpose()?;
    let locked_fields_raw: String = row
        .try_get("", "locked_fields")
        .map_err(|e| e.to_string())?;
    let locked_fields: Vec<String> = serde_json::from_str(&locked_fields_raw)
        .map_err(|error| format!("model_prices[{model_id}] invalid locked_fields: {error}"))?;
    let raw_json_raw: String = row.try_get("", "raw_json").map_err(|e| e.to_string())?;
    // MP-D5: a malformed raw_json fails the read; it must not decode as `{}`.
    let raw_json: serde_json::Value = serde_json::from_str(&raw_json_raw)
        .map_err(|error| format!("model_prices[{model_id}] invalid raw_json: {error}"))?;
    if !raw_json.is_object() {
        return Err(format!("model_prices[{model_id}] raw_json is not an object"));
    }
    let updated_at_raw: String = row.try_get("", "updated_at").map_err(|e| e.to_string())?;
    Ok(ModelPriceRecord {
        billing_mode: row.try_get("", "billing_mode").map_err(|e| e.to_string())?,
        input_usd_per_1m: row
            .try_get("", "input_usd_per_1m")
            .map_err(|e| e.to_string())?,
        output_usd_per_1m: row
            .try_get("", "output_usd_per_1m")
            .map_err(|e| e.to_string())?,
        cache_read_usd_per_1m: row
            .try_get("", "cache_read_usd_per_1m")
            .map_err(|e| e.to_string())?,
        cache_write_usd_per_1m: row
            .try_get("", "cache_write_usd_per_1m")
            .map_err(|e| e.to_string())?,
        cache_write_1h_usd_per_1m: row
            .try_get("", "cache_write_1h_usd_per_1m")
            .map_err(|e| e.to_string())?,
        reasoning_usd_per_1m: row
            .try_get("", "reasoning_usd_per_1m")
            .map_err(|e| e.to_string())?,
        per_request_usd: row
            .try_get("", "per_request_usd")
            .map_err(|e| e.to_string())?,
        billing_expr,
        source: row.try_get("", "source").map_err(|e| e.to_string())?,
        locked_fields,
        raw_json,
        enabled: row.try_get::<i32>("", "enabled").map_err(|e| e.to_string())? != 0,
        updated_at: parse_time(&updated_at_raw, "updated_at")?,
        model_id,
    })
}

#[derive(Clone)]
pub struct ModelPriceStore {
    db: DbPool,
}

impl ModelPriceStore {
    pub fn new(db: DbPool) -> Self {
        Self { db }
    }

    /// MP-A1: all rows ordered by `model_id ASC`.
    pub async fn list(&self) -> Result<Vec<ModelPriceRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!("SELECT {MODEL_PRICE_COLUMNS} FROM model_prices ORDER BY model_id ASC"),
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(row_to_record).collect()
    }

    /// MP-R8: load candidate rows for all distinct pricing keys of one
    /// forwarding request in set-based queries (chunked below the portable
    /// SQLite bound-parameter limit).
    pub async fn list_by_model_ids(
        &self,
        model_ids: &[String],
    ) -> Result<Vec<ModelPriceRecord>, String> {
        let mut records = Vec::new();
        for chunk in model_ids.chunks(100) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = (1..=chunk.len())
                .map(|index| format!("${index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let rows = self
                .db
                .read()
                .query_all(self.db.stmt(
                    &format!(
                        "SELECT {MODEL_PRICE_COLUMNS} FROM model_prices \
                         WHERE model_id IN ({placeholders})"
                    ),
                    chunk.iter().map(|id| id.clone().into()).collect(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            for row in &rows {
                records.push(row_to_record(row)?);
            }
        }
        Ok(records)
    }

    pub async fn get(&self, model_id: &str) -> Result<Option<ModelPriceRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                &format!("SELECT {MODEL_PRICE_COLUMNS} FROM model_prices WHERE model_id = $1"),
                vec![model_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        match row {
            Some(row) => Ok(Some(row_to_record(&row)?)),
            None => Ok(None),
        }
    }

    /// MP-A2 merge-upsert with MP-Y17 lock-on-edit semantics. Returns the
    /// stored row after the write. Validation errors are returned as `Err`
    /// with a user-facing message; the caller maps them to HTTP 400.
    pub async fn upsert(
        &self,
        model_id: &str,
        input: UpsertModelPriceInput,
    ) -> Result<ModelPriceRecord, String> {
        let model_id = model_id.trim();
        if model_id.is_empty() || model_id.chars().count() > 256 {
            return Err("model_id must be 1-256 characters".to_string());
        }

        let existing = self.get(model_id).await?;
        let mut record = existing.clone().unwrap_or_else(|| ModelPriceRecord {
            model_id: model_id.to_string(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: None,
            output_usd_per_1m: None,
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            source: "manual".to_string(),
            locked_fields: Vec::new(),
            raw_json: serde_json::json!({}),
            enabled: true,
            updated_at: Utc::now(),
        });

        let mut changed_price_fields: Vec<&'static str> = Vec::new();
        if let Some(mode) = &input.billing_mode {
            if !BILLING_MODES.contains(&mode.as_str()) {
                return Err(
                    "billing_mode must be one of per_token, per_request, tiered_expr".to_string(),
                );
            }
            if *mode != record.billing_mode {
                changed_price_fields.push("billing_mode");
            }
            record.billing_mode = mode.clone();
        }
        macro_rules! merge_price {
            ($field:ident, $name:literal) => {
                if let Some(value) = &input.$field {
                    validate_price_option($name, value)?;
                    if *value != record.$field {
                        changed_price_fields.push($name);
                    }
                    record.$field = value.clone();
                }
            };
        }
        merge_price!(input_usd_per_1m, "input_usd_per_1m");
        merge_price!(output_usd_per_1m, "output_usd_per_1m");
        merge_price!(cache_read_usd_per_1m, "cache_read_usd_per_1m");
        merge_price!(cache_write_usd_per_1m, "cache_write_usd_per_1m");
        merge_price!(cache_write_1h_usd_per_1m, "cache_write_1h_usd_per_1m");
        merge_price!(reasoning_usd_per_1m, "reasoning_usd_per_1m");
        merge_price!(per_request_usd, "per_request_usd");
        if let Some(expr) = &input.billing_expr {
            if let Some(expr) = expr {
                validate_billing_expr(expr)?;
            }
            if *expr != record.billing_expr {
                changed_price_fields.push("billing_expr");
            }
            record.billing_expr = expr.clone();
        }
        if let Some(enabled) = input.enabled {
            record.enabled = enabled;
        }

        // MP-Y17: a dashboard write locks the edited price fields and sets
        // `source = "manual"` only for a previously absent row; an existing
        // synced row keeps its source. MP-Y18: an explicit locked_fields
        // replaces the lock set instead.
        if existing.is_none() {
            record.source = "manual".to_string();
        }
        match input.locked_fields {
            Some(fields) => {
                validate_locked_fields(&fields)?;
                record.locked_fields = fields;
            }
            None => {
                for field in changed_price_fields {
                    if !record.locked_fields.iter().any(|locked| locked == field) {
                        record.locked_fields.push(field.to_string());
                    }
                }
            }
        }

        record.updated_at = Utc::now();
        let locked_fields_json =
            serde_json::to_string(&record.locked_fields).map_err(|e| e.to_string())?;
        let billing_expr_json = record
            .billing_expr
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;
        let raw_json = serde_json::to_string(&record.raw_json).map_err(|e| e.to_string())?;

        let sql = if existing.is_some() {
            "UPDATE model_prices SET billing_mode = $2, input_usd_per_1m = $3, \
             output_usd_per_1m = $4, cache_read_usd_per_1m = $5, cache_write_usd_per_1m = $6, \
             cache_write_1h_usd_per_1m = $7, reasoning_usd_per_1m = $8, per_request_usd = $9, \
             billing_expr = $10, source = $11, locked_fields = $12, raw_json = $13, \
             enabled = $14, updated_at = $15 WHERE model_id = $1"
        } else {
            "INSERT INTO model_prices (model_id, billing_mode, input_usd_per_1m, \
             output_usd_per_1m, cache_read_usd_per_1m, cache_write_usd_per_1m, \
             cache_write_1h_usd_per_1m, reasoning_usd_per_1m, per_request_usd, billing_expr, \
             source, locked_fields, raw_json, enabled, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"
        };
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                sql,
                vec![
                    record.model_id.clone().into(),
                    record.billing_mode.clone().into(),
                    record.input_usd_per_1m.clone().into(),
                    record.output_usd_per_1m.clone().into(),
                    record.cache_read_usd_per_1m.clone().into(),
                    record.cache_write_usd_per_1m.clone().into(),
                    record.cache_write_1h_usd_per_1m.clone().into(),
                    record.reasoning_usd_per_1m.clone().into(),
                    record.per_request_usd.clone().into(),
                    billing_expr_json.into(),
                    record.source.clone().into(),
                    locked_fields_json.into(),
                    raw_json.into(),
                    SeaValue::Int(Some(if record.enabled { 1 } else { 0 })),
                    record.updated_at.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(record)
    }

    /// MP-A3: `Ok(false)` when the row does not exist.
    pub async fn delete(&self, model_id: &str) -> Result<bool, String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM model_prices WHERE model_id = $1",
                vec![model_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    /// MP-Y13/MP-Y16: set-based upsert of fully merged sync rows in
    /// fixed-size chunks below the portable SQLite bound-parameter limit.
    /// Callers resolve ownership and locks before this write.
    pub async fn bulk_upsert_synced(&self, rows: &[ModelPriceRecord]) -> Result<(), String> {
        // 15 bound values per row; 60 rows keeps every statement below the
        // portable 999-parameter SQLite bound.
        const CHUNK_SIZE: usize = 60;
        let write_guard = self.db.write().await;
        for chunk in rows.chunks(CHUNK_SIZE) {
            let mut values: Vec<SeaValue> = Vec::with_capacity(chunk.len() * 15);
            let mut tuples = Vec::with_capacity(chunk.len());
            for record in chunk {
                let start = values.len() + 1;
                let locked_fields_json =
                    serde_json::to_string(&record.locked_fields).map_err(|e| e.to_string())?;
                let billing_expr_json = record
                    .billing_expr
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let raw_json =
                    serde_json::to_string(&record.raw_json).map_err(|e| e.to_string())?;
                values.extend([
                    record.model_id.clone().into(),
                    record.billing_mode.clone().into(),
                    record.input_usd_per_1m.clone().into(),
                    record.output_usd_per_1m.clone().into(),
                    record.cache_read_usd_per_1m.clone().into(),
                    record.cache_write_usd_per_1m.clone().into(),
                    record.cache_write_1h_usd_per_1m.clone().into(),
                    record.reasoning_usd_per_1m.clone().into(),
                    record.per_request_usd.clone().into(),
                    billing_expr_json.into(),
                    record.source.clone().into(),
                    locked_fields_json.into(),
                    raw_json.into(),
                    SeaValue::Int(Some(if record.enabled { 1 } else { 0 })),
                    record.updated_at.to_rfc3339().into(),
                ]);
                let placeholders = (start..start + 15)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                tuples.push(format!("({placeholders})"));
            }
            if tuples.is_empty() {
                continue;
            }
            write_guard
                .execute(self.db.stmt(
                    &format!(
                        "INSERT INTO model_prices ({MODEL_PRICE_COLUMNS}) VALUES {} \
                         ON CONFLICT(model_id) DO UPDATE SET \
                           billing_mode=excluded.billing_mode, \
                           input_usd_per_1m=excluded.input_usd_per_1m, \
                           output_usd_per_1m=excluded.output_usd_per_1m, \
                           cache_read_usd_per_1m=excluded.cache_read_usd_per_1m, \
                           cache_write_usd_per_1m=excluded.cache_write_usd_per_1m, \
                           cache_write_1h_usd_per_1m=excluded.cache_write_1h_usd_per_1m, \
                           reasoning_usd_per_1m=excluded.reasoning_usd_per_1m, \
                           per_request_usd=excluded.per_request_usd, \
                           billing_expr=excluded.billing_expr, \
                           source=excluded.source, \
                           locked_fields=excluded.locked_fields, \
                           raw_json=excluded.raw_json, \
                           enabled=excluded.enabled, \
                           updated_at=excluded.updated_at",
                        tuples.join(", ")
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// MP-Y15: chunked delete of source-owned rows by model id.
    pub async fn delete_by_ids_with_source(
        &self,
        model_ids: &[String],
        source: &str,
    ) -> Result<u64, String> {
        let mut deleted = 0u64;
        let write_guard = self.db.write().await;
        for chunk in model_ids.chunks(100) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = (2..=chunk.len() + 1)
                .map(|index| format!("${index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut values: Vec<SeaValue> = vec![source.into()];
            values.extend(chunk.iter().map(|id| SeaValue::from(id.clone())));
            let result = write_guard
                .execute(self.db.stmt(
                    &format!(
                        "DELETE FROM model_prices WHERE source = $1 \
                         AND model_id IN ({placeholders})"
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
            deleted += result.rows_affected();
        }
        Ok(deleted)
    }

    /// MP-Y16: inserts one run row with `status = "running"`.
    pub async fn insert_sync_run(&self, source: &str) -> Result<PriceSyncRun, String> {
        let run = PriceSyncRun {
            id: uuid::Uuid::new_v4().to_string(),
            source: source.to_string(),
            status: "running".to_string(),
            started_at: Utc::now(),
            finished_at: None,
            inserted: 0,
            updated: 0,
            skipped: 0,
            deleted: 0,
            error: None,
            detail_json: serde_json::json!({}),
        };
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "INSERT INTO price_sync_runs \
                 (id, source, status, started_at, finished_at, inserted, updated, skipped, \
                  deleted, error, detail_json) \
                 VALUES ($1, $2, $3, $4, NULL, 0, 0, 0, 0, NULL, '{}')",
                vec![
                    run.id.clone().into(),
                    run.source.clone().into(),
                    run.status.clone().into(),
                    run.started_at.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(run)
    }

    /// MP-Y16: finalizes a run row with counts and returns the stored row.
    #[allow(clippy::too_many_arguments)]
    pub async fn finalize_sync_run(
        &self,
        id: &str,
        status: &str,
        inserted: i32,
        updated: i32,
        skipped: i32,
        deleted: i32,
        error: Option<&str>,
        detail_json: &serde_json::Value,
    ) -> Result<PriceSyncRun, String> {
        let finished_at = Utc::now();
        let detail = serde_json::to_string(detail_json).map_err(|e| e.to_string())?;
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "UPDATE price_sync_runs SET status = $2, finished_at = $3, inserted = $4, \
                 updated = $5, skipped = $6, deleted = $7, error = $8, detail_json = $9 \
                 WHERE id = $1",
                vec![
                    id.into(),
                    status.into(),
                    finished_at.to_rfc3339().into(),
                    SeaValue::Int(Some(inserted)),
                    SeaValue::Int(Some(updated)),
                    SeaValue::Int(Some(skipped)),
                    SeaValue::Int(Some(deleted)),
                    error.map(|e| e.to_string()).into(),
                    detail.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, source, status, started_at, finished_at, inserted, updated, \
                 skipped, deleted, error, detail_json FROM price_sync_runs WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "sync run row missing after finalize".to_string())?;
        row_to_sync_run(&row)
    }

    /// MP-A5: most recent runs first, bounded limit.
    pub async fn list_sync_runs(&self, limit: u64) -> Result<Vec<PriceSyncRun>, String> {
        let limit = limit.clamp(1, 100);
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, source, status, started_at, finished_at, inserted, updated, \
                     skipped, deleted, error, detail_json FROM price_sync_runs \
                     ORDER BY started_at DESC LIMIT {limit}"
                ),
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(row_to_sync_run).collect()
    }
}

fn row_to_sync_run(row: &QueryResult) -> Result<PriceSyncRun, String> {
    let started_at_raw: String = row.try_get("", "started_at").map_err(|e| e.to_string())?;
    let finished_at_raw: Option<String> =
        row.try_get("", "finished_at").map_err(|e| e.to_string())?;
    let detail_raw: String = row.try_get("", "detail_json").map_err(|e| e.to_string())?;
    Ok(PriceSyncRun {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        source: row.try_get("", "source").map_err(|e| e.to_string())?,
        status: row.try_get("", "status").map_err(|e| e.to_string())?,
        started_at: parse_time(&started_at_raw, "started_at")?,
        finished_at: finished_at_raw
            .map(|raw| parse_time(&raw, "finished_at"))
            .transpose()?,
        inserted: row.try_get("", "inserted").map_err(|e| e.to_string())?,
        updated: row.try_get("", "updated").map_err(|e| e.to_string())?,
        skipped: row.try_get("", "skipped").map_err(|e| e.to_string())?,
        deleted: row.try_get("", "deleted").map_err(|e| e.to_string())?,
        error: row.try_get("", "error").map_err(|e| e.to_string())?,
        detail_json: serde_json::from_str(&detail_raw)
            .map_err(|error| format!("invalid detail_json: {error}"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::validate_billing_expr;
    use serde_json::json;

    #[test]
    fn billing_expr_accepts_valid_tier_tables() {
        let expr = json!({
            "tiers": [
                { "when_input_tokens_lte": 200000, "input_usd_per_1m": "1.25",
                  "output_usd_per_1m": "10" },
                { "input_usd_per_1m": "2.5", "output_usd_per_1m": "15" }
            ]
        });
        assert!(validate_billing_expr(&expr).is_ok());
    }

    #[test]
    fn billing_expr_rejects_structural_violations() {
        for expr in [
            json!({}),
            json!({ "tiers": [] }),
            // Non-last tier without a bound.
            json!({ "tiers": [
                { "input_usd_per_1m": "1" },
                { "input_usd_per_1m": "2" }
            ] }),
            // Last tier with a bound.
            json!({ "tiers": [ { "when_input_tokens_lte": 5, "input_usd_per_1m": "1" } ] }),
            // Bounds not strictly increasing.
            json!({ "tiers": [
                { "when_input_tokens_lte": 10, "input_usd_per_1m": "1" },
                { "when_input_tokens_lte": 10, "input_usd_per_1m": "2" },
                { "input_usd_per_1m": "3" }
            ] }),
            // Missing input price.
            json!({ "tiers": [ { "output_usd_per_1m": "1" } ] }),
            // Numeric price instead of decimal string.
            json!({ "tiers": [ { "input_usd_per_1m": 1.25 } ] }),
        ] {
            assert!(validate_billing_expr(&expr).is_err(), "accepted {expr}");
        }
    }
}

//! `model_prices` and `price_sync_runs` persistence (`model-pricing.spec.md` §2, §10).

mod sync;
pub use sync::SyncRunOutcome;

use crate::db::DbPool;
use crate::db_cache::REQUEST_PATH_CACHE_TTL;
use crate::settings::validate_usd_decimal;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use sea_orm::{ConnectionTrait, QueryResult, Value as SeaValue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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

pub(crate) fn is_service_tier_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() || key.len() > 32 {
        return false;
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && !matches!(key, "default" | "auto" | "standard" | "priority")
}

fn validate_tiers_array(tiers: &[serde_json::Value], path: &str) -> Result<(), String> {
    if tiers.is_empty() || tiers.len() > 8 {
        return Err(format!("{path} must contain 1 to 8 tiers"));
    }
    let mut previous_bound: Option<u64> = None;
    for (index, tier) in tiers.iter().enumerate() {
        let tier = tier
            .as_object()
            .ok_or_else(|| format!("{path}[{index}] must be an object"))?;
        let is_last = index == tiers.len() - 1;
        match tier.get("when_input_tokens_lte") {
            Some(bound) if !is_last => {
                let bound = bound.as_u64().filter(|value| *value >= 1).ok_or_else(|| {
                    format!("{path}[{index}].when_input_tokens_lte must be an integer >= 1")
                })?;
                if previous_bound.is_some_and(|previous| bound <= previous) {
                    return Err(format!("{path} tier bounds must be strictly increasing"));
                }
                previous_bound = Some(bound);
            }
            Some(serde_json::Value::Null) | None if is_last => {}
            Some(_) => {
                return Err(format!(
                    "the last {path} tier must omit when_input_tokens_lte"
                ));
            }
            None => {
                return Err(format!("{path}[{index}] must set when_input_tokens_lte"));
            }
        }
        let mut has_input_price = false;
        for (key, value) in tier {
            if key == "when_input_tokens_lte" {
                continue;
            }
            if !TIER_PRICE_FIELDS.contains(&key.as_str()) {
                return Err(format!("{path}[{index}]: unknown field `{key}`"));
            }
            match value {
                serde_json::Value::Null => {}
                serde_json::Value::String(raw) => {
                    validate_usd_decimal(raw)
                        .map_err(|message| format!("{path}[{index}].{key}: {message}"))?;
                    if key == "input_usd_per_1m" {
                        has_input_price = true;
                    }
                }
                _ => {
                    return Err(format!(
                        "{path}[{index}].{key} must be a decimal string or null"
                    ));
                }
            }
        }
        if !has_input_price {
            return Err(format!(
                "{path}[{index}] must set a non-null input_usd_per_1m"
            ));
        }
    }
    Ok(())
}

/// MP-C6/MP-C6a/MP-C7 write-time validation for `billing_expr`.
pub fn validate_billing_expr(expr: &serde_json::Value) -> Result<(), String> {
    let object = expr
        .as_object()
        .ok_or_else(|| "billing_expr must be a JSON object".to_string())?;
    for key in object.keys() {
        if key != "tiers" && key != "service_tiers" {
            return Err(format!("billing_expr: unknown field `{key}`"));
        }
    }
    let tiers = object
        .get("tiers")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "billing_expr.tiers must be an array".to_string())?;
    validate_tiers_array(tiers, "billing_expr.tiers")?;
    match object.get("service_tiers") {
        None => Ok(()),
        Some(serde_json::Value::Object(service_tiers)) => {
            if service_tiers.is_empty() || service_tiers.len() > 8 {
                return Err("billing_expr.service_tiers must contain 1 to 8 keys".to_string());
            }
            for (name, table) in service_tiers {
                if !is_service_tier_key(name) {
                    return Err(format!("billing_expr.service_tiers: invalid key `{name}`"));
                }
                let table = table.as_object().ok_or_else(|| {
                    format!("billing_expr.service_tiers.{name} must be an object")
                })?;
                for key in table.keys() {
                    if key != "tiers" {
                        return Err(format!(
                            "billing_expr.service_tiers.{name}: unknown field `{key}`"
                        ));
                    }
                }
                let tiers = table
                    .get("tiers")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| {
                        format!("billing_expr.service_tiers.{name}.tiers must be an array")
                    })?;
                validate_tiers_array(tiers, &format!("billing_expr.service_tiers.{name}.tiers"))?;
            }
            Ok(())
        }
        Some(_) => Err("billing_expr.service_tiers must be an object".to_string()),
    }
}

pub fn billing_expr_has_service_tiers(expr: &Option<serde_json::Value>) -> bool {
    expr.as_ref()
        .and_then(|value| value.get("service_tiers"))
        .and_then(|value| value.as_object())
        .is_some_and(|object| !object.is_empty())
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
        return Err(format!(
            "model_prices[{model_id}] raw_json is not an object"
        ));
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
        enabled: row
            .try_get::<i32>("", "enabled")
            .map_err(|e| e.to_string())?
            != 0,
        updated_at: parse_time(&updated_at_raw, "updated_at")?,
        model_id,
    })
}

/// DPT-RC5: the applicable row for one pricing key, or its absence.
struct CachedModelPrice {
    record: Option<Arc<ModelPriceRecord>>,
    read_started_at: Instant,
    generation: u64,
}

const MODEL_PRICE_CACHE_MAX_ENTRIES: usize = 4096;

#[derive(Default)]
struct ModelPriceCache {
    entries: DashMap<String, CachedModelPrice>,
    generation: AtomicU64,
}

impl ModelPriceCache {
    fn servable(&self, entry: &CachedModelPrice) -> bool {
        entry.generation == self.generation.load(Ordering::Acquire)
            && entry.read_started_at.elapsed() <= REQUEST_PATH_CACHE_TTL
    }

    fn publish(
        &self,
        model_id: String,
        record: Option<Arc<ModelPriceRecord>>,
        read_started_at: Instant,
        generation: u64,
    ) {
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        if !self.entries.contains_key(&model_id)
            && self.entries.len() >= MODEL_PRICE_CACHE_MAX_ENTRIES
        {
            self.entries.retain(|_, entry| self.servable(entry));
            if self.entries.len() >= MODEL_PRICE_CACHE_MAX_ENTRIES {
                return;
            }
        }
        // Readers also compare generations, so an entry published just
        // before a concurrent invalidation is never served.
        self.entries.insert(
            model_id,
            CachedModelPrice {
                record,
                read_started_at,
                generation,
            },
        );
    }

    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.entries.clear();
    }
}

#[derive(Clone)]
pub struct ModelPriceStore {
    db: DbPool,
    price_cache: Arc<ModelPriceCache>,
}

fn merge_upsert_input(
    record: &mut ModelPriceRecord,
    input: UpsertModelPriceInput,
    is_new: bool,
) -> Result<(), String> {
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
    if record.billing_mode != "tiered_expr" && billing_expr_has_service_tiers(&record.billing_expr)
    {
        return Err("service_tiers require billing_mode tiered_expr".to_string());
    }
    if let Some(enabled) = input.enabled {
        record.enabled = enabled;
    }

    // MP-Y17: a dashboard write locks the edited price fields and sets
    // `source = "manual"` only for a previously absent row; an existing
    // synced row keeps its source. MP-Y18: an explicit locked_fields
    // replaces the lock set instead.
    if is_new {
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

    Ok(())
}

impl ModelPriceStore {
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            price_cache: Arc::default(),
        }
    }

    /// DPT-RC5: applicable rows (MP-R2/MP-R4) for the given pricing keys.
    /// Keys without a servable cache entry are read with one set-based
    /// `list_by_model_ids` call.
    pub async fn list_applicable_by_model_ids(
        &self,
        model_ids: &[String],
    ) -> Result<HashMap<String, Arc<ModelPriceRecord>>, String> {
        let cache = &self.price_cache;
        let mut applicable = HashMap::with_capacity(model_ids.len());
        let mut missing = Vec::new();
        for model_id in model_ids {
            match cache
                .entries
                .get(model_id)
                .filter(|entry| cache.servable(entry))
            {
                Some(entry) => {
                    if let Some(record) = &entry.record {
                        applicable.insert(model_id.clone(), Arc::clone(record));
                    }
                }
                None => missing.push(model_id.clone()),
            }
        }
        if missing.is_empty() {
            return Ok(applicable);
        }
        missing.sort_unstable();
        missing.dedup();
        let generation = cache.generation.load(Ordering::Acquire);
        let read_started_at = Instant::now();
        let mut rows = self
            .list_by_model_ids(&missing)
            .await?
            .into_iter()
            .map(|row| (row.model_id.clone(), row))
            .collect::<HashMap<_, _>>();
        for model_id in missing {
            let record = rows
                .remove(&model_id)
                .filter(|row| row.enabled && row.is_complete())
                .map(Arc::new);
            if let Some(record) = &record {
                applicable.insert(model_id.clone(), Arc::clone(record));
            }
            cache.publish(model_id, record, read_started_at, generation);
        }
        Ok(applicable)
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

        merge_upsert_input(&mut record, input, existing.is_none())?;

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
        let result = self
            .db
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
            .await;
        self.price_cache.invalidate();
        result.map_err(|e| e.to_string())?;
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
            .await;
        self.price_cache.invalidate();
        Ok(result.map_err(|e| e.to_string())?.rows_affected() > 0)
    }
}

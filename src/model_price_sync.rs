use crate::bounded_response::read_upstream_discovery_body;
use crate::model_price_store::{
    MODEL_PRICE_COLUMNS, ModelPriceRecord, ModelPriceStore, PriceSyncRun, row_to_record,
};
use crate::model_registry_store::normalize_model_id;
use crate::settings::{SystemSettings, validate_usd_decimal};
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use sea_orm::{ConnectionTrait, Value as SeaValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);
const PRICE_WRITE_CHUNK_SIZE: usize = 50;
const METADATA_WRITE_CHUNK_SIZE: usize = 50;
const MAX_PREVIEW_CHANGES: usize = 500;
const MAX_DETAIL_JSON_BYTES: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSyncSource {
    ModelsDev,
    Openrouter,
    NewApi,
}

impl PriceSyncSource {
    pub fn parse(raw: &str) -> Result<Self, PriceSyncError> {
        match raw {
            "models_dev" => Ok(Self::ModelsDev),
            "openrouter" => Ok(Self::Openrouter),
            "new_api" => Ok(Self::NewApi),
            _ => Err(PriceSyncError::InvalidRequest(format!(
                "unsupported price sync source: {raw}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelsDev => "models_dev",
            Self::Openrouter => "openrouter",
            Self::NewApi => "new_api",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PriceSyncError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    Upstream(String),
    #[error("{0}")]
    Storage(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceSyncChange {
    pub model_id: String,
    pub kind: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceSyncPreview {
    pub source: String,
    pub insert: usize,
    pub update: usize,
    pub skip: usize,
    pub delete: usize,
    pub changes: Vec<PriceSyncChange>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct IncomingPrice {
    model_id: String,
    billing_mode: String,
    input_usd_per_1m: Option<String>,
    output_usd_per_1m: Option<String>,
    cache_read_usd_per_1m: Option<String>,
    cache_write_usd_per_1m: Option<String>,
    cache_write_1h_usd_per_1m: Option<String>,
    reasoning_usd_per_1m: Option<String>,
    per_request_usd: Option<String>,
    billing_expr: Option<Value>,
    raw_json: Value,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct IncomingMetadata {
    model_id: String,
    models_dev_provider: String,
    mode: String,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw_json: Value,
}

#[derive(Debug, Default)]
struct SyncSnapshot {
    prices: BTreeMap<String, IncomingPrice>,
    metadata: Vec<IncomingMetadata>,
}

#[derive(Debug)]
struct SyncPlan {
    preview: PriceSyncPreview,
    upserts: Vec<ModelPriceRecord>,
    deletes: Vec<String>,
}

#[derive(Debug, Clone)]
struct ModelsDevVariant {
    provider_id: String,
    family: Option<String>,
    input: Option<Decimal>,
    output: Option<Decimal>,
    cache_read: Option<Decimal>,
    cache_write: Option<Decimal>,
    reasoning: Option<Decimal>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw: Value,
}

impl ModelPriceStore {
    pub async fn preview_sync(
        &self,
        http: &reqwest::Client,
        source: PriceSyncSource,
        settings: &SystemSettings,
    ) -> Result<PriceSyncPreview, PriceSyncError> {
        let snapshot = fetch_snapshot(http, source, settings).await?;
        let current = load_current_prices(&self.db, self.db.read()).await?;
        Ok(build_sync_plan(source, &snapshot, current, Utc::now()).preview)
    }

    pub async fn apply_sync(
        &self,
        http: &reqwest::Client,
        source: PriceSyncSource,
        settings: &SystemSettings,
    ) -> Result<PriceSyncRun, PriceSyncError> {
        let id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now();
        insert_running_sync(&self.db, &id, source, started_at).await?;

        let snapshot = match fetch_snapshot(http, source, settings).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                finalize_failed_sync(&self.db, &id, &error.to_string()).await?;
                return Err(error);
            }
        };

        let txn = self
            .db
            .begin_write()
            .await
            .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
        let current = match load_current_prices(&self.db, &*txn).await {
            Ok(current) => current,
            Err(error) => {
                txn.rollback()
                    .await
                    .map_err(|rollback| PriceSyncError::Storage(rollback.to_string()))?;
                finalize_failed_sync(&self.db, &id, &error.to_string()).await?;
                return Err(error);
            }
        };
        let now = Utc::now();
        let plan = build_sync_plan(source, &snapshot, current, now);

        let apply_result = async {
            write_price_upserts(&self.db, &*txn, &plan.upserts).await?;
            write_price_deletes(&self.db, &*txn, &plan.deletes).await?;
            if source == PriceSyncSource::ModelsDev {
                replace_models_dev_metadata(&self.db, &*txn, &snapshot.metadata, now).await?;
            }
            let detail_json = bounded_detail_json(&plan.preview)?;
            finalize_success_sync(&self.db, &*txn, &id, &plan.preview, now, &detail_json).await?;
            Ok::<(), PriceSyncError>(())
        }
        .await;

        if let Err(error) = apply_result {
            txn.rollback()
                .await
                .map_err(|rollback| PriceSyncError::Storage(rollback.to_string()))?;
            finalize_failed_sync(&self.db, &id, &error.to_string()).await?;
            return Err(error);
        }
        txn.commit()
            .await
            .map_err(|error| PriceSyncError::Storage(error.to_string()))?;

        Ok(PriceSyncRun {
            id,
            source: source.as_str().to_string(),
            status: "success".to_string(),
            started_at,
            finished_at: Some(now),
            inserted: count_i32(plan.preview.insert)?,
            updated: count_i32(plan.preview.update)?,
            skipped: count_i32(plan.preview.skip)?,
            deleted: count_i32(plan.preview.delete)?,
            error: None,
            detail_json: bounded_detail_json(&plan.preview)?,
        })
    }
}

async fn fetch_snapshot(
    http: &reqwest::Client,
    source: PriceSyncSource,
    settings: &SystemSettings,
) -> Result<SyncSnapshot, PriceSyncError> {
    let (url, bearer) = match source {
        PriceSyncSource::ModelsDev => (MODELS_DEV_URL.to_string(), None),
        PriceSyncSource::Openrouter => (OPENROUTER_URL.to_string(), None),
        PriceSyncSource::NewApi => {
            let base = settings
                .price_sync_new_api_base_url
                .trim()
                .trim_end_matches('/');
            if base.is_empty() {
                return Err(PriceSyncError::InvalidRequest(
                    "new-api price sync is disabled because its base URL is empty".to_string(),
                ));
            }
            let token = settings.price_sync_new_api_token.trim();
            (
                format!("{base}/api/pricing"),
                (!token.is_empty()).then(|| token.to_string()),
            )
        }
    };

    let mut request = http.get(&url).timeout(SYNC_TIMEOUT);
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| PriceSyncError::Upstream(format!("fetch_failed: {error}")))?;
    if !response.status().is_success() {
        return Err(PriceSyncError::Upstream(format!(
            "fetch_failed: status={}",
            response.status()
        )));
    }
    let body = read_upstream_discovery_body(response)
        .await
        .map_err(|error| PriceSyncError::Upstream(format!("fetch_failed: {error}")))?;
    let root: Value = serde_json::from_slice(&body)
        .map_err(|error| PriceSyncError::Upstream(format!("parse_failed: {error}")))?;
    match source {
        PriceSyncSource::ModelsDev => parse_models_dev(root),
        PriceSyncSource::Openrouter => parse_openrouter(root),
        PriceSyncSource::NewApi => parse_new_api(root),
    }
}

fn parse_models_dev(root: Value) -> Result<SyncSnapshot, PriceSyncError> {
    let providers = root.as_object().ok_or_else(|| {
        PriceSyncError::Upstream("parse_failed: models.dev root must be an object".to_string())
    })?;
    let mut grouped: BTreeMap<String, Vec<ModelsDevVariant>> = BTreeMap::new();
    for (provider_id, provider_value) in providers {
        let Some(models) = provider_value
            .as_object()
            .and_then(|provider| provider.get("models"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (model_name, model_value) in models {
            let Some(model) = model_value.as_object() else {
                continue;
            };
            let canonical = normalize_model_id(model_name, Some(provider_id));
            if should_skip_model(&canonical) {
                continue;
            }
            let cost = model.get("cost").and_then(Value::as_object);
            let limit = model.get("limit").and_then(Value::as_object);
            grouped
                .entry(canonical)
                .or_default()
                .push(ModelsDevVariant {
                    provider_id: provider_id.clone(),
                    family: model
                        .get("family")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    input: cost
                        .and_then(|cost| cost.get("input"))
                        .and_then(decimal_value),
                    output: cost
                        .and_then(|cost| cost.get("output"))
                        .and_then(decimal_value),
                    cache_read: cost
                        .and_then(|cost| cost.get("cache_read"))
                        .and_then(decimal_value),
                    cache_write: cost
                        .and_then(|cost| cost.get("cache_write"))
                        .and_then(decimal_value),
                    reasoning: cost
                        .and_then(|cost| cost.get("reasoning"))
                        .and_then(decimal_value),
                    max_input_tokens: limit
                        .and_then(|limit| limit.get("input"))
                        .and_then(value_i64),
                    max_output_tokens: limit
                        .and_then(|limit| limit.get("output"))
                        .and_then(value_i64),
                    max_tokens: limit
                        .and_then(|limit| limit.get("context"))
                        .and_then(value_i64),
                    raw: stringify_cost_values(model_value.clone()),
                });
        }
    }

    let mut snapshot = SyncSnapshot::default();
    for (model_id, variants) in grouped {
        let Some(winner) = select_models_dev_variant(&model_id, &variants) else {
            continue;
        };
        let mut providers = Map::new();
        for variant in &variants {
            providers.insert(variant.provider_id.clone(), variant.raw.clone());
        }
        let raw_json = json!({ "providers": providers });
        snapshot.prices.insert(
            model_id.clone(),
            IncomingPrice {
                model_id: model_id.clone(),
                billing_mode: "per_token".to_string(),
                input_usd_per_1m: decimal_string(winner.input),
                output_usd_per_1m: decimal_string(winner.output),
                cache_read_usd_per_1m: decimal_string(winner.cache_read),
                cache_write_usd_per_1m: decimal_string(winner.cache_write),
                cache_write_1h_usd_per_1m: None,
                reasoning_usd_per_1m: decimal_string(winner.reasoning),
                per_request_usd: None,
                billing_expr: None,
                raw_json: raw_json.clone(),
                enabled: true,
            },
        );
        snapshot.metadata.push(IncomingMetadata {
            model_id,
            models_dev_provider: winner.provider_id.clone(),
            mode: if variants.iter().any(|variant| {
                variant
                    .family
                    .as_deref()
                    .is_some_and(|family| family.to_ascii_lowercase().contains("embed"))
            }) {
                "embedding".to_string()
            } else {
                "chat".to_string()
            },
            max_input_tokens: winner.max_input_tokens,
            max_output_tokens: winner.max_output_tokens,
            max_tokens: winner.max_tokens,
            raw_json,
        });
    }
    Ok(snapshot)
}

fn parse_openrouter(root: Value) -> Result<SyncSnapshot, PriceSyncError> {
    let entries = root
        .as_object()
        .and_then(|object| object.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            PriceSyncError::Upstream(
                "parse_failed: OpenRouter response must contain a data array".to_string(),
            )
        })?;
    let mut snapshot = SyncSnapshot::default();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        let model_id = normalize_model_id(id, None);
        if should_skip_model(&model_id) {
            continue;
        }
        let pricing = object.get("pricing").and_then(Value::as_object);
        let input = pricing
            .and_then(|pricing| pricing.get("prompt"))
            .and_then(decimal_value)
            .and_then(|value| value.checked_mul(Decimal::from(1_000_000_u64)));
        let output = pricing
            .and_then(|pricing| pricing.get("completion"))
            .and_then(decimal_value)
            .and_then(|value| value.checked_mul(Decimal::from(1_000_000_u64)));
        if !input.is_some_and(|value| value > Decimal::ZERO)
            && !output.is_some_and(|value| value > Decimal::ZERO)
        {
            continue;
        }
        let candidate = IncomingPrice {
            model_id: model_id.clone(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: decimal_string(input),
            output_usd_per_1m: decimal_string(output),
            cache_read_usd_per_1m: decimal_string(
                pricing
                    .and_then(|pricing| pricing.get("input_cache_read"))
                    .and_then(decimal_value)
                    .and_then(|value| value.checked_mul(Decimal::from(1_000_000_u64))),
            ),
            cache_write_usd_per_1m: decimal_string(
                pricing
                    .and_then(|pricing| pricing.get("input_cache_write"))
                    .and_then(decimal_value)
                    .and_then(|value| value.checked_mul(Decimal::from(1_000_000_u64))),
            ),
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            raw_json: stringify_object_numbers(entry.clone()),
            enabled: true,
        };
        let replace = snapshot
            .prices
            .get(&model_id)
            .and_then(|current| current.input_usd_per_1m.as_deref())
            .and_then(|raw| Decimal::from_str(raw).ok())
            .is_none_or(|current| input.unwrap_or(Decimal::ZERO) > current);
        if replace {
            snapshot.prices.insert(model_id, candidate);
        }
    }
    Ok(snapshot)
}

fn parse_new_api(root: Value) -> Result<SyncSnapshot, PriceSyncError> {
    let entries = root
        .as_array()
        .or_else(|| {
            root.as_object()
                .and_then(|object| object.get("data"))
                .and_then(Value::as_array)
        })
        .ok_or_else(|| {
            PriceSyncError::Upstream(
                "parse_failed: new-api response must be an array or contain a data array"
                    .to_string(),
            )
        })?;
    let mut snapshot = SyncSnapshot::default();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(name) = object.get("model_name").and_then(Value::as_str) else {
            continue;
        };
        let model_id = normalize_model_id(name, None);
        if should_skip_model(&model_id) {
            continue;
        }
        let quota_type = object
            .get("quota_type")
            .and_then(value_i64)
            .unwrap_or_default();
        let mut price = IncomingPrice {
            model_id: model_id.clone(),
            billing_mode: String::new(),
            input_usd_per_1m: None,
            output_usd_per_1m: None,
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            raw_json: stringify_object_numbers(entry.clone()),
            enabled: true,
        };
        match quota_type {
            0 => {
                let Some(model_ratio) = object.get("model_ratio").and_then(decimal_value) else {
                    continue;
                };
                if model_ratio < Decimal::ZERO {
                    continue;
                }
                let Some(input) = model_ratio.checked_mul(Decimal::from(2_u32)) else {
                    continue;
                };
                let completion_ratio = object
                    .get("completion_ratio")
                    .and_then(decimal_value)
                    .unwrap_or(Decimal::ONE);
                let Some(output) = input.checked_mul(completion_ratio) else {
                    continue;
                };
                price.billing_mode = "per_token".to_string();
                price.input_usd_per_1m = decimal_string(Some(input));
                price.output_usd_per_1m = decimal_string(Some(output));
            }
            1 => {
                let Some(per_request) = object.get("model_price").and_then(decimal_value) else {
                    continue;
                };
                if per_request < Decimal::ZERO {
                    continue;
                }
                price.billing_mode = "per_request".to_string();
                price.per_request_usd = decimal_string(Some(per_request));
            }
            _ => continue,
        }
        snapshot.prices.insert(model_id, price);
    }
    Ok(snapshot)
}

fn build_sync_plan(
    source: PriceSyncSource,
    snapshot: &SyncSnapshot,
    current: BTreeMap<String, ModelPriceRecord>,
    now: DateTime<Utc>,
) -> SyncPlan {
    let mut preview = PriceSyncPreview {
        source: source.as_str().to_string(),
        insert: 0,
        update: 0,
        skip: 0,
        delete: 0,
        changes: Vec::new(),
        truncated: false,
    };
    let mut upserts = Vec::new();
    for incoming in snapshot.prices.values() {
        match current.get(&incoming.model_id) {
            None => {
                preview.insert += 1;
                push_change(
                    &mut preview,
                    PriceSyncChange {
                        model_id: incoming.model_id.clone(),
                        kind: "insert".to_string(),
                        fields: incoming_field_names(incoming),
                    },
                );
                upserts.push(incoming_record(incoming, source, now));
            }
            Some(existing) if existing.source != source.as_str() => {
                preview.skip += 1;
                push_change(
                    &mut preview,
                    PriceSyncChange {
                        model_id: incoming.model_id.clone(),
                        kind: "skip".to_string(),
                        fields: Vec::new(),
                    },
                );
            }
            Some(existing) => {
                let (merged, changed, blocked) = merge_synced_record(existing, incoming, now);
                if blocked {
                    preview.skip += 1;
                }
                if !changed.is_empty() {
                    preview.update += 1;
                    push_change(
                        &mut preview,
                        PriceSyncChange {
                            model_id: incoming.model_id.clone(),
                            kind: "update".to_string(),
                            fields: changed,
                        },
                    );
                }
                upserts.push(merged);
            }
        }
    }

    let deletes = if source == PriceSyncSource::ModelsDev {
        current
            .values()
            .filter(|row| {
                row.source == source.as_str() && !snapshot.prices.contains_key(&row.model_id)
            })
            .map(|row| row.model_id.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    preview.delete = deletes.len();
    for model_id in &deletes {
        push_change(
            &mut preview,
            PriceSyncChange {
                model_id: model_id.clone(),
                kind: "delete".to_string(),
                fields: Vec::new(),
            },
        );
    }
    SyncPlan {
        preview,
        upserts,
        deletes,
    }
}

fn incoming_record(
    incoming: &IncomingPrice,
    source: PriceSyncSource,
    now: DateTime<Utc>,
) -> ModelPriceRecord {
    ModelPriceRecord {
        model_id: incoming.model_id.clone(),
        billing_mode: incoming.billing_mode.clone(),
        input_usd_per_1m: incoming.input_usd_per_1m.clone(),
        output_usd_per_1m: incoming.output_usd_per_1m.clone(),
        cache_read_usd_per_1m: incoming.cache_read_usd_per_1m.clone(),
        cache_write_usd_per_1m: incoming.cache_write_usd_per_1m.clone(),
        cache_write_1h_usd_per_1m: incoming.cache_write_1h_usd_per_1m.clone(),
        reasoning_usd_per_1m: incoming.reasoning_usd_per_1m.clone(),
        per_request_usd: incoming.per_request_usd.clone(),
        billing_expr: incoming.billing_expr.clone(),
        source: source.as_str().to_string(),
        locked_fields: Vec::new(),
        raw_json: incoming.raw_json.clone(),
        enabled: incoming.enabled,
        updated_at: now,
    }
}

fn merge_synced_record(
    existing: &ModelPriceRecord,
    incoming: &IncomingPrice,
    now: DateTime<Utc>,
) -> (ModelPriceRecord, Vec<String>, bool) {
    let locks = existing
        .locked_fields
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut merged = existing.clone();
    let mut changed = Vec::new();
    let mut blocked = false;
    macro_rules! merge_field {
        ($field:ident, $name:literal) => {
            if existing.$field != incoming.$field {
                if locks.contains($name) {
                    blocked = true;
                } else {
                    merged.$field = incoming.$field.clone();
                    changed.push($name.to_string());
                }
            }
        };
    }
    merge_field!(billing_mode, "billing_mode");
    merge_field!(input_usd_per_1m, "input_usd_per_1m");
    merge_field!(output_usd_per_1m, "output_usd_per_1m");
    merge_field!(cache_read_usd_per_1m, "cache_read_usd_per_1m");
    merge_field!(cache_write_usd_per_1m, "cache_write_usd_per_1m");
    merge_field!(cache_write_1h_usd_per_1m, "cache_write_1h_usd_per_1m");
    merge_field!(reasoning_usd_per_1m, "reasoning_usd_per_1m");
    merge_field!(per_request_usd, "per_request_usd");
    merge_field!(billing_expr, "billing_expr");
    if existing.enabled != incoming.enabled {
        if locks.contains("enabled") {
            blocked = true;
        } else {
            merged.enabled = incoming.enabled;
            changed.push("enabled".to_string());
        }
    }
    if existing.raw_json != incoming.raw_json {
        merged.raw_json = incoming.raw_json.clone();
        changed.push("raw_json".to_string());
    }
    merged.updated_at = now;
    (merged, changed, blocked)
}

fn incoming_field_names(incoming: &IncomingPrice) -> Vec<String> {
    let mut fields = vec!["billing_mode".to_string(), "enabled".to_string()];
    for (name, present) in [
        ("input_usd_per_1m", incoming.input_usd_per_1m.is_some()),
        ("output_usd_per_1m", incoming.output_usd_per_1m.is_some()),
        (
            "cache_read_usd_per_1m",
            incoming.cache_read_usd_per_1m.is_some(),
        ),
        (
            "cache_write_usd_per_1m",
            incoming.cache_write_usd_per_1m.is_some(),
        ),
        (
            "cache_write_1h_usd_per_1m",
            incoming.cache_write_1h_usd_per_1m.is_some(),
        ),
        (
            "reasoning_usd_per_1m",
            incoming.reasoning_usd_per_1m.is_some(),
        ),
        ("per_request_usd", incoming.per_request_usd.is_some()),
        ("billing_expr", incoming.billing_expr.is_some()),
    ] {
        if present {
            fields.push(name.to_string());
        }
    }
    fields
}

fn push_change(preview: &mut PriceSyncPreview, change: PriceSyncChange) {
    if preview.changes.len() < MAX_PREVIEW_CHANGES {
        preview.changes.push(change);
    } else {
        preview.truncated = true;
    }
}

async fn load_current_prices<C: ConnectionTrait>(
    db: &crate::db::DbPool,
    conn: &C,
) -> Result<BTreeMap<String, ModelPriceRecord>, PriceSyncError> {
    let rows = conn
        .query_all(db.stmt(
            &format!("SELECT {MODEL_PRICE_COLUMNS} FROM model_prices ORDER BY model_id ASC"),
            vec![],
        ))
        .await
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    rows.iter()
        .map(|row| {
            let record = row_to_record(row).map_err(PriceSyncError::Storage)?;
            Ok((record.model_id.clone(), record))
        })
        .collect()
}

async fn write_price_upserts<C: ConnectionTrait>(
    db: &crate::db::DbPool,
    conn: &C,
    rows: &[ModelPriceRecord],
) -> Result<(), PriceSyncError> {
    for chunk in rows.chunks(PRICE_WRITE_CHUNK_SIZE) {
        let mut values = Vec::with_capacity(chunk.len() * 15);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for row in chunk {
            let start = values.len() + 1;
            values.extend([
                row.model_id.clone().into(),
                row.billing_mode.clone().into(),
                row.input_usd_per_1m.clone().into(),
                row.output_usd_per_1m.clone().into(),
                row.cache_read_usd_per_1m.clone().into(),
                row.cache_write_usd_per_1m.clone().into(),
                row.cache_write_1h_usd_per_1m.clone().into(),
                row.reasoning_usd_per_1m.clone().into(),
                row.per_request_usd.clone().into(),
                row.billing_expr.as_ref().map(Value::to_string).into(),
                row.source.clone().into(),
                serde_json::to_string(&row.locked_fields)
                    .map_err(|error| PriceSyncError::Storage(error.to_string()))?
                    .into(),
                row.raw_json.to_string().into(),
                SeaValue::Int(Some(if row.enabled { 1 } else { 0 })),
                row.updated_at.to_rfc3339().into(),
            ]);
            placeholders.push(format!(
                "({})",
                (start..start + 15)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        conn.execute(db.stmt(
            &format!(
                "INSERT INTO model_prices
                 (model_id, billing_mode, input_usd_per_1m, output_usd_per_1m,
                  cache_read_usd_per_1m, cache_write_usd_per_1m,
                  cache_write_1h_usd_per_1m, reasoning_usd_per_1m, per_request_usd,
                  billing_expr, source, locked_fields, raw_json, enabled, updated_at)
                 VALUES {}
                 ON CONFLICT(model_id) DO UPDATE SET
                   billing_mode=excluded.billing_mode,
                   input_usd_per_1m=excluded.input_usd_per_1m,
                   output_usd_per_1m=excluded.output_usd_per_1m,
                   cache_read_usd_per_1m=excluded.cache_read_usd_per_1m,
                   cache_write_usd_per_1m=excluded.cache_write_usd_per_1m,
                   cache_write_1h_usd_per_1m=excluded.cache_write_1h_usd_per_1m,
                   reasoning_usd_per_1m=excluded.reasoning_usd_per_1m,
                   per_request_usd=excluded.per_request_usd,
                   billing_expr=excluded.billing_expr,
                   source=excluded.source,
                   locked_fields=excluded.locked_fields,
                   raw_json=excluded.raw_json,
                   enabled=excluded.enabled,
                   updated_at=excluded.updated_at",
                placeholders.join(", ")
            ),
            values,
        ))
        .await
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    }
    Ok(())
}

async fn write_price_deletes<C: ConnectionTrait>(
    db: &crate::db::DbPool,
    conn: &C,
    model_ids: &[String],
) -> Result<(), PriceSyncError> {
    for chunk in model_ids.chunks(500) {
        let placeholders = (1..=chunk.len())
            .map(|index| format!("${index}"))
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute(db.stmt(
            &format!("DELETE FROM model_prices WHERE model_id IN ({placeholders})"),
            chunk.iter().cloned().map(Into::into).collect(),
        ))
        .await
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    }
    Ok(())
}

async fn replace_models_dev_metadata<C: ConnectionTrait>(
    db: &crate::db::DbPool,
    conn: &C,
    metadata: &[IncomingMetadata],
    now: DateTime<Utc>,
) -> Result<(), PriceSyncError> {
    conn.execute(db.stmt(
        "DELETE FROM model_metadata_records WHERE source != 'manual'",
        vec![],
    ))
    .await
    .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    let manual_rows = conn
        .query_all(db.stmt(
            "SELECT model_id FROM model_metadata_records WHERE source = 'manual'",
            vec![],
        ))
        .await
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    let manual_ids = manual_rows
        .iter()
        .map(|row| row.try_get::<String>("", "model_id"))
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;

    for chunk in metadata
        .iter()
        .filter(|row| !manual_ids.contains(&row.model_id))
        .collect::<Vec<_>>()
        .chunks(METADATA_WRITE_CHUNK_SIZE)
    {
        let mut values = Vec::with_capacity(chunk.len() * 9);
        let mut placeholders = Vec::with_capacity(chunk.len());
        for row in chunk {
            let start = values.len() + 1;
            values.extend([
                row.model_id.clone().into(),
                row.models_dev_provider.clone().into(),
                row.mode.clone().into(),
                row.max_input_tokens.into(),
                row.max_output_tokens.into(),
                row.max_tokens.into(),
                row.raw_json.to_string().into(),
                "models_dev".into(),
                now.to_rfc3339().into(),
            ]);
            placeholders.push(format!(
                "({})",
                (start..start + 9)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        conn.execute(db.stmt(
            &format!(
                "INSERT INTO model_metadata_records
                 (model_id, models_dev_provider, mode, max_input_tokens,
                  max_output_tokens, max_tokens, raw_json, source, updated_at)
                 VALUES {}
                 ON CONFLICT(model_id) DO UPDATE SET
                   models_dev_provider=excluded.models_dev_provider,
                   mode=excluded.mode,
                   max_input_tokens=excluded.max_input_tokens,
                   max_output_tokens=excluded.max_output_tokens,
                   max_tokens=excluded.max_tokens,
                   raw_json=excluded.raw_json,
                   source=excluded.source,
                   updated_at=excluded.updated_at",
                placeholders.join(", ")
            ),
            values,
        ))
        .await
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    }
    Ok(())
}

async fn insert_running_sync(
    db: &crate::db::DbPool,
    id: &str,
    source: PriceSyncSource,
    started_at: DateTime<Utc>,
) -> Result<(), PriceSyncError> {
    db.write()
        .await
        .execute(db.stmt(
            "INSERT INTO price_sync_runs
             (id, source, status, started_at, finished_at, inserted, updated,
              skipped, deleted, error, detail_json)
             VALUES ($1, $2, 'running', $3, NULL, 0, 0, 0, 0, NULL, '{}')",
            vec![
                id.to_string().into(),
                source.as_str().into(),
                started_at.to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    Ok(())
}

async fn finalize_failed_sync(
    db: &crate::db::DbPool,
    id: &str,
    error: &str,
) -> Result<(), PriceSyncError> {
    db.write()
        .await
        .execute(db.stmt(
            "UPDATE price_sync_runs
             SET status = 'failed', finished_at = $2, error = $3
             WHERE id = $1",
            vec![
                id.to_string().into(),
                Utc::now().to_rfc3339().into(),
                error.chars().take(4096).collect::<String>().into(),
            ],
        ))
        .await
        .map_err(|storage| PriceSyncError::Storage(storage.to_string()))?;
    Ok(())
}

async fn finalize_success_sync<C: ConnectionTrait>(
    db: &crate::db::DbPool,
    conn: &C,
    id: &str,
    preview: &PriceSyncPreview,
    finished_at: DateTime<Utc>,
    detail_json: &Value,
) -> Result<(), PriceSyncError> {
    conn.execute(db.stmt(
        "UPDATE price_sync_runs
         SET status = 'success', finished_at = $2, inserted = $3, updated = $4,
             skipped = $5, deleted = $6, error = NULL, detail_json = $7
         WHERE id = $1",
        vec![
            id.to_string().into(),
            finished_at.to_rfc3339().into(),
            count_i32(preview.insert)?.into(),
            count_i32(preview.update)?.into(),
            count_i32(preview.skip)?.into(),
            count_i32(preview.delete)?.into(),
            detail_json.to_string().into(),
        ],
    ))
    .await
    .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    Ok(())
}

fn bounded_detail_json(preview: &PriceSyncPreview) -> Result<Value, PriceSyncError> {
    let mut detail = serde_json::to_value(preview)
        .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
    loop {
        let encoded = serde_json::to_vec(&detail)
            .map_err(|error| PriceSyncError::Storage(error.to_string()))?;
        if encoded.len() <= MAX_DETAIL_JSON_BYTES {
            return Ok(detail);
        }
        let Some(object) = detail.as_object_mut() else {
            return Err(PriceSyncError::Storage(
                "price sync detail is not an object".to_string(),
            ));
        };
        object.insert("truncated".to_string(), Value::Bool(true));
        let Some(changes) = object.get_mut("changes").and_then(Value::as_array_mut) else {
            return Err(PriceSyncError::Storage(
                "price sync detail has no changes array".to_string(),
            ));
        };
        if changes.pop().is_none() {
            return Err(PriceSyncError::Storage(
                "price sync detail exceeds its limit without changes".to_string(),
            ));
        }
    }
}

fn count_i32(value: usize) -> Result<i32, PriceSyncError> {
    i32::try_from(value)
        .map_err(|_| PriceSyncError::Storage("price sync count exceeds i32".to_string()))
}

fn decimal_value(value: &Value) -> Option<Decimal> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(raw) => raw.clone(),
        _ => return None,
    };
    if raw.is_empty() || raw.starts_with('+') || raw.contains(['e', 'E']) {
        return None;
    }
    Decimal::from_str(&raw).ok()
}

fn decimal_string(value: Option<Decimal>) -> Option<String> {
    let value = value?;
    if value < Decimal::ZERO {
        return None;
    }
    let value = value.round_dp_with_strategy(9, RoundingStrategy::ToZero);
    let raw = value.normalize().to_string();
    validate_usd_decimal(&raw).ok()?;
    Some(raw)
}

fn stringify_cost_values(mut value: Value) -> Value {
    let Some(cost) = value
        .as_object_mut()
        .and_then(|object| object.get_mut("cost"))
        .and_then(Value::as_object_mut)
    else {
        return value;
    };
    for price in cost.values_mut() {
        if let Value::Number(number) = price {
            *price = Value::String(number.to_string());
        }
    }
    value
}

fn stringify_object_numbers(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        stringify_numbers_in_object(object);
    }
    value
}

fn stringify_numbers_in_object(object: &mut Map<String, Value>) {
    for value in object.values_mut() {
        match value {
            Value::Number(number) => *value = Value::String(number.to_string()),
            Value::Object(nested) => stringify_numbers_in_object(nested),
            Value::Array(items) => {
                for item in items {
                    if let Value::Object(nested) = item {
                        stringify_numbers_in_object(nested);
                    }
                }
            }
            _ => {}
        }
    }
}

fn value_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn select_models_dev_variant<'a>(
    model_id: &str,
    variants: &'a [ModelsDevVariant],
) -> Option<&'a ModelsDevVariant> {
    let positive =
        |variant: &&ModelsDevVariant| variant.input.is_some_and(|price| price > Decimal::ZERO);
    if let Some(official) = official_provider_for_model(model_id)
        && let Some(variant) = variants
            .iter()
            .filter(positive)
            .find(|variant| variant.provider_id == official)
    {
        return Some(variant);
    }
    variants
        .iter()
        .filter(positive)
        .max_by_key(|variant| variant.input.unwrap_or(Decimal::ZERO))
}

fn official_provider_for_model(model_id: &str) -> Option<&'static str> {
    let openai_o =
        model_id.starts_with('o') && model_id.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    if model_id.starts_with("gpt-") || openai_o || model_id.starts_with("chatgpt-") {
        Some("openai")
    } else if model_id.starts_with("claude-") {
        Some("anthropic")
    } else if model_id.starts_with("gemini-") || model_id.starts_with("gemma-") {
        Some("google")
    } else if model_id.starts_with("grok-") {
        Some("xai")
    } else if model_id.starts_with("deepseek-") {
        Some("deepseek")
    } else if [
        "mistral-",
        "codestral-",
        "pixtral-",
        "ministral-",
        "magistral-",
        "devstral-",
    ]
    .iter()
    .any(|prefix| model_id.starts_with(prefix))
    {
        Some("mistral")
    } else if model_id.starts_with("qwen")
        || model_id.starts_with("qwq-")
        || model_id.starts_with("qvq-")
    {
        Some("alibaba")
    } else if model_id.starts_with("llama-") {
        Some("llama")
    } else if model_id.starts_with("command-") {
        Some("cohere")
    } else if model_id.starts_with("kimi-") || model_id.starts_with("moonshot-") {
        Some("moonshotai")
    } else if model_id.starts_with("glm-") {
        Some("zhipuai")
    } else if model_id.starts_with("minimax-") {
        Some("minimax")
    } else if model_id.starts_with("step-") {
        Some("stepfun")
    } else if model_id.starts_with("sonar") {
        Some("perplexity")
    } else if model_id.starts_with("solar-") {
        Some("upstage")
    } else if model_id.starts_with("phi-") {
        Some("azure")
    } else if model_id.starts_with("mimo-") {
        Some("xiaomi")
    } else if model_id.starts_with("mercury") {
        Some("inception")
    } else {
        None
    }
}

fn should_skip_model(model_id: &str) -> bool {
    model_id == "auto"
        || model_id.ends_with("-thinking")
        || model_id.ends_with(":thinking")
        || model_id.ends_with("-think")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_dev_prefers_official_variant_and_keeps_exact_decimal_strings() {
        let snapshot = parse_models_dev(json!({
            "openai": { "models": { "gpt-4o": {
                "cost": { "input": 2.500000001, "output": 10 },
                "limit": { "context": 128000 }
            } } },
            "openrouter": { "models": { "openai/gpt-4o": {
                "cost": { "input": 9, "output": 18 }
            } } }
        }))
        .unwrap();
        let row = snapshot.prices.get("gpt-4o").unwrap();
        assert_eq!(row.input_usd_per_1m.as_deref(), Some("2.500000001"));
        assert_eq!(row.output_usd_per_1m.as_deref(), Some("10"));
        assert_eq!(
            row.raw_json["providers"]["openai"]["cost"]["input"],
            "2.500000001"
        );
    }

    #[test]
    fn openrouter_and_new_api_mapping_use_exact_decimal_arithmetic() {
        let openrouter = parse_openrouter(json!({ "data": [{
            "id": "vendor/model-a",
            "pricing": { "prompt": "0.00000125", "completion": "0.00001" }
        }] }))
        .unwrap();
        let row = openrouter.prices.get("model-a").unwrap();
        assert_eq!(row.input_usd_per_1m.as_deref(), Some("1.25"));
        assert_eq!(row.output_usd_per_1m.as_deref(), Some("10"));

        let new_api = parse_new_api(json!({ "data": [{
            "model_name": "model-b", "quota_type": 0,
            "model_ratio": "1.25", "completion_ratio": "3.5"
        }] }))
        .unwrap();
        let row = new_api.prices.get("model-b").unwrap();
        assert_eq!(row.input_usd_per_1m.as_deref(), Some("2.5"));
        assert_eq!(row.output_usd_per_1m.as_deref(), Some("8.75"));
    }

    #[test]
    fn sync_plan_preserves_locked_fields_and_synced_source() {
        let now = Utc::now();
        let existing = ModelPriceRecord {
            model_id: "model-a".to_string(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: Some("1".to_string()),
            output_usd_per_1m: Some("2".to_string()),
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            source: "openrouter".to_string(),
            locked_fields: vec!["input_usd_per_1m".to_string()],
            raw_json: json!({}),
            enabled: true,
            updated_at: now,
        };
        let incoming = IncomingPrice {
            model_id: "model-a".to_string(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: Some("9".to_string()),
            output_usd_per_1m: Some("8".to_string()),
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            raw_json: json!({ "fresh": true }),
            enabled: true,
        };
        let (merged, changed, blocked) = merge_synced_record(&existing, &incoming, now);
        assert!(blocked);
        assert_eq!(merged.source, "openrouter");
        assert_eq!(merged.input_usd_per_1m.as_deref(), Some("1"));
        assert_eq!(merged.output_usd_per_1m.as_deref(), Some("8"));
        assert!(changed.contains(&"output_usd_per_1m".to_string()));
        assert!(changed.contains(&"raw_json".to_string()));
    }
}

//! External price sync engine (`model-pricing.spec.md` §9).
//!
//! Three sources map upstream price catalogs into `model_prices` rows:
//! `models_dev`, `openrouter`, and `new_api`. Preview computes the diff
//! without writes (MP-A6); apply performs the ownership-scoped upsert with
//! field locks and an audit run row (MP-Y13..MP-Y16).

use crate::model_price_store::{
    ModelPriceRecord, ModelPriceStore, PriceSyncRun, validate_billing_expr,
};
use crate::model_registry_store::{
    group_models_dev_variants, has_positive_input_variant, normalize_model_id, pick_best_variant,
    usd_cost_string,
};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// MP-Y3: fetch timeout for every sync source.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// MP-A6: `changes` is truncated to at most 500 entries.
const MAX_CHANGE_ENTRIES: usize = 500;

/// MP-D8: `detail_json` byte bound after serialization.
const MAX_DETAIL_BYTES: usize = 262_144;

const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const AUTO_SYNC_SETTING_POLL_INTERVAL: Duration = Duration::from_secs(60);

static APPLY_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn apply_lock() -> &'static tokio::sync::Mutex<()> {
    APPLY_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncSource {
    ModelsDev,
    OpenRouter,
    NewApi,
}

impl SyncSource {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "models_dev" => Some(Self::ModelsDev),
            "openrouter" => Some(Self::OpenRouter),
            "new_api" => Some(Self::NewApi),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ModelsDev => "models_dev",
            Self::OpenRouter => "openrouter",
            Self::NewApi => "new_api",
        }
    }
}

/// One mapped price candidate produced by a source fetch. Every price value
/// is an exact USD decimal string per MP-U1.
#[derive(Debug, Clone)]
pub struct SyncCandidate {
    pub model_id: String,
    pub billing_mode: String,
    pub input_usd_per_1m: Option<String>,
    pub output_usd_per_1m: Option<String>,
    pub cache_read_usd_per_1m: Option<String>,
    pub cache_write_usd_per_1m: Option<String>,
    pub reasoning_usd_per_1m: Option<String>,
    pub per_request_usd: Option<String>,
    pub billing_expr: Option<Value>,
    pub raw_json: Value,
}

/// A fetched and mapped source snapshot. `models_dev_root` carries the parsed
/// models.dev document for the MP-Y10 metadata upsert.
pub struct SourceSnapshot {
    pub candidates: Vec<SyncCandidate>,
    pub models_dev_root: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncChange {
    pub model_id: String,
    pub kind: &'static str,
    pub fields: Vec<String>,
}

/// The computed apply plan for one source snapshot against the stored rows.
pub struct SyncPlan {
    pub inserted: i32,
    pub updated: i32,
    pub skipped: i32,
    pub deleted: i32,
    pub changes: Vec<SyncChange>,
    pub truncated: bool,
    /// Fully merged rows to write (inserts, updates, and owned rows whose
    /// raw_json refreshes without a counted change, per MP-Y14).
    pub writes: Vec<ModelPriceRecord>,
    /// `models_dev` rows absent from the snapshot (MP-Y15).
    pub delete_ids: Vec<String>,
}

/// Fetches `https://models.dev/api.json` and returns the parsed root.
/// Errors carry a `fetch_failed:`/`parse_failed:` prefix for HTTP mapping.
pub async fn fetch_models_dev_root(http: &reqwest::Client) -> Result<Value, String> {
    fetch_json(http, "https://models.dev/api.json", None).await
}

async fn fetch_json(
    http: &reqwest::Client,
    url: &str,
    bearer_token: Option<&str>,
) -> Result<Value, String> {
    let mut request = http.get(url).timeout(FETCH_TIMEOUT);
    if let Some(token) = bearer_token.filter(|token| !token.is_empty()) {
        request = request.bearer_auth(token);
    }
    let resp = request
        .send()
        .await
        .map_err(|e| format!("fetch_failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("fetch_failed: status={status}"));
    }
    let body = crate::bounded_response::read_upstream_discovery_body(resp)
        .await
        .map_err(|e| format!("fetch_failed: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("parse_failed: {e}"))
}

/// Fetches and maps one source into price candidates. `new_api_config` is
/// `(base_url, token)` from system settings; an empty base URL disables the
/// `new_api` source (MP-Y2).
pub async fn fetch_source_snapshot(
    http: &reqwest::Client,
    source: SyncSource,
    new_api_config: (&str, &str),
) -> Result<SourceSnapshot, String> {
    match source {
        SyncSource::ModelsDev => {
            let root = fetch_models_dev_root(http).await?;
            let candidates = map_models_dev(&root)?;
            Ok(SourceSnapshot {
                candidates,
                models_dev_root: Some(root),
            })
        }
        SyncSource::OpenRouter => {
            let root = fetch_json(http, "https://openrouter.ai/api/v1/models", None).await?;
            Ok(SourceSnapshot {
                candidates: map_openrouter(&root)?,
                models_dev_root: None,
            })
        }
        SyncSource::NewApi => {
            let (base_url, token) = new_api_config;
            let base_url = base_url.trim_end_matches('/');
            if base_url.is_empty() {
                return Err("source_disabled: price_sync_new_api_base_url is not set".to_string());
            }
            let root = fetch_json(http, &format!("{base_url}/api/pricing"), Some(token)).await?;
            Ok(SourceSnapshot {
                candidates: map_new_api(&root)?,
                models_dev_root: None,
            })
        }
    }
}

/// MP-Y4..MP-Y9: models.dev grouped-variant mapping.
pub fn map_models_dev(root: &Value) -> Result<Vec<SyncCandidate>, String> {
    let grouped = group_models_dev_variants(root)?;
    let mut candidates = Vec::new();
    for (model_id, variants) in &grouped {
        // MP-Y7: models without any strictly positive input cost are skipped.
        if !has_positive_input_variant(variants) {
            continue;
        }
        let winner = pick_best_variant(model_id, variants);
        let mut providers_map = serde_json::Map::new();
        for variant in variants {
            providers_map.insert(variant.provider_id.clone(), variant.raw.clone());
        }
        let billing_expr = models_dev_billing_expr(model_id, winner)?;
        candidates.push(SyncCandidate {
            model_id: model_id.clone(),
            billing_mode: if billing_expr.is_some() {
                "tiered_expr".to_string()
            } else {
                "per_token".to_string()
            },
            input_usd_per_1m: winner.cost_input.clone(),
            output_usd_per_1m: winner.cost_output.clone(),
            cache_read_usd_per_1m: winner.cost_cache_read.clone(),
            cache_write_usd_per_1m: winner.cost_cache_write.clone(),
            reasoning_usd_per_1m: winner.cost_reasoning.clone(),
            per_request_usd: None,
            billing_expr,
            raw_json: json!({ "providers": Value::Object(providers_map) }),
        });
    }
    Ok(candidates)
}

fn put_tier_price(
    target: &mut serde_json::Map<String, Value>,
    target_field: &str,
    raw: Option<&Value>,
    model_id: &str,
) -> Result<(), String> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let value = usd_cost_string(raw).ok_or_else(|| {
        format!("parse_failed: models.dev model {model_id} has invalid tier price {target_field}")
    })?;
    target.insert(target_field.to_string(), Value::String(value));
    Ok(())
}

fn models_dev_billing_expr(
    model_id: &str,
    winner: &crate::model_registry_store::SyncProviderVariant,
) -> Result<Option<Value>, String> {
    let Some(cost) = winner.raw.get("cost").and_then(Value::as_object) else {
        return Ok(None);
    };
    let source_tiers = match cost.get("tiers") {
        None => return Ok(None),
        Some(Value::Array(tiers)) => tiers,
        Some(_) => {
            return Err(format!(
                "parse_failed: models.dev model {model_id} cost.tiers is not an array"
            ));
        }
    };
    if source_tiers.is_empty() {
        return Ok(None);
    }
    if source_tiers.len() > 7 {
        return Err(format!(
            "parse_failed: models.dev model {model_id} has more than 7 cost tiers"
        ));
    }

    let mut parsed = Vec::with_capacity(source_tiers.len());
    for (index, source_tier) in source_tiers.iter().enumerate() {
        let tier = source_tier.as_object().ok_or_else(|| {
            format!(
                "parse_failed: models.dev model {model_id} cost.tiers[{index}] is not an object"
            )
        })?;
        let selector = tier.get("tier").and_then(Value::as_object).ok_or_else(|| {
            format!("parse_failed: models.dev model {model_id} cost.tiers[{index}].tier is not an object")
        })?;
        if selector.get("type").and_then(Value::as_str) != Some("context") {
            return Err(format!(
                "parse_failed: models.dev model {model_id} cost.tiers[{index}] is not a context tier"
            ));
        }
        let size = selector
            .get("size")
            .and_then(Value::as_u64)
            .filter(|size| *size >= 1)
            .ok_or_else(|| {
                format!(
                    "parse_failed: models.dev model {model_id} cost.tiers[{index}].tier.size is not an integer >= 1"
                )
            })?;
        let mut mapped = serde_json::Map::new();
        for (source, target) in [
            ("input", "input_usd_per_1m"),
            ("output", "output_usd_per_1m"),
            ("cache_read", "cache_read_usd_per_1m"),
            ("cache_write", "cache_write_usd_per_1m"),
            ("reasoning", "reasoning_usd_per_1m"),
        ] {
            put_tier_price(&mut mapped, target, tier.get(source), model_id)?;
        }
        if !mapped.contains_key("input_usd_per_1m") {
            return Err(format!(
                "parse_failed: models.dev model {model_id} cost.tiers[{index}] has no input price"
            ));
        }
        parsed.push((size, mapped));
    }
    parsed.sort_by_key(|(size, _)| *size);
    if parsed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(format!(
            "parse_failed: models.dev model {model_id} has duplicate cost tier sizes"
        ));
    }

    let mut base = serde_json::Map::new();
    base.insert(
        "when_input_tokens_lte".to_string(),
        Value::from(parsed[0].0),
    );
    for (field, value) in [
        ("input_usd_per_1m", winner.cost_input.as_ref()),
        ("output_usd_per_1m", winner.cost_output.as_ref()),
        ("cache_read_usd_per_1m", winner.cost_cache_read.as_ref()),
        ("cache_write_usd_per_1m", winner.cost_cache_write.as_ref()),
        ("reasoning_usd_per_1m", winner.cost_reasoning.as_ref()),
    ] {
        if let Some(value) = value {
            base.insert(field.to_string(), Value::String(value.clone()));
        }
    }

    let mut tiers = vec![Value::Object(base)];
    for index in 0..parsed.len() {
        let mut tier = parsed[index].1.clone();
        if let Some((next_size, _)) = parsed.get(index + 1) {
            tier.insert("when_input_tokens_lte".to_string(), Value::from(*next_size));
        }
        tiers.push(Value::Object(tier));
    }
    let expr = json!({ "tiers": tiers });
    validate_billing_expr(&expr).map_err(|error| {
        format!("parse_failed: models.dev model {model_id} produced invalid billing_expr: {error}")
    })?;
    Ok(Some(expr))
}

fn per_token_to_per_1m(value: &Value) -> Option<String> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    let per_token = Decimal::from_str_exact(&raw).ok()?;
    if per_token.is_sign_negative() {
        return None;
    }
    let per_1m = per_token.checked_mul(Decimal::from(1_000_000u32))?;
    Some(
        per_1m
            .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
            .normalize()
            .to_string(),
    )
}

fn decimal_is_positive(raw: Option<&String>) -> bool {
    raw.and_then(|value| Decimal::from_str(value).ok())
        .is_some_and(|value| value.is_sign_positive() && !value.is_zero())
}

/// MP-Y11: OpenRouter per-token USD strings scale to USD per 1M exactly.
/// A model with non-positive prompt and completion prices is skipped. When
/// two entries normalize to the same canonical id, the higher prompt price
/// wins (same resale-loss rationale as MP-Y7).
pub fn map_openrouter(root: &Value) -> Result<Vec<SyncCandidate>, String> {
    let data = root
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "parse_failed: data must be an array".to_string())?;
    let mut by_model: BTreeMap<String, SyncCandidate> = BTreeMap::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(|id| id.as_str()) else {
            continue;
        };
        let vendor = id.split('/').next().filter(|vendor| *vendor != id);
        let model_id = normalize_model_id(id, vendor);
        let pricing = entry.get("pricing").and_then(|pricing| pricing.as_object());
        let input = pricing
            .and_then(|pricing| pricing.get("prompt"))
            .and_then(per_token_to_per_1m);
        let output = pricing
            .and_then(|pricing| pricing.get("completion"))
            .and_then(per_token_to_per_1m);
        if !decimal_is_positive(input.as_ref()) && !decimal_is_positive(output.as_ref()) {
            continue;
        }
        let candidate = SyncCandidate {
            model_id: model_id.clone(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: input,
            output_usd_per_1m: output,
            cache_read_usd_per_1m: pricing
                .and_then(|pricing| pricing.get("input_cache_read"))
                .and_then(per_token_to_per_1m),
            cache_write_usd_per_1m: pricing
                .and_then(|pricing| pricing.get("input_cache_write"))
                .and_then(per_token_to_per_1m),
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            raw_json: entry.clone(),
        };
        match by_model.entry(model_id) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let existing = decimal_value(slot.get().input_usd_per_1m.as_ref());
                let incoming = decimal_value(candidate.input_usd_per_1m.as_ref());
                if incoming > existing {
                    slot.insert(candidate);
                }
            }
        }
    }
    Ok(by_model.into_values().collect())
}

fn decimal_value(raw: Option<&String>) -> Decimal {
    raw.and_then(|value| Decimal::from_str(value).ok())
        .unwrap_or(Decimal::ZERO)
}

fn json_decimal(value: Option<&Value>) -> Option<Decimal> {
    let raw = match value? {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    Decimal::from_str_exact(&raw).ok()
}

fn trunc9(value: Decimal) -> String {
    value
        .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
        .normalize()
        .to_string()
}

/// MP-Y12/MP-Y12a: new-api `/api/pricing` mapping with untrusted-placeholder
/// skip. Ratio `1` equals USD 2 per 1M tokens; all arithmetic is exact
/// decimal truncated to 9 fractional digits.
pub fn map_new_api(root: &Value) -> Result<Vec<SyncCandidate>, String> {
    let data = root
        .get("data")
        .and_then(|data| data.as_array())
        .or_else(|| root.as_array())
        .ok_or_else(|| "parse_failed: data must be an array".to_string())?;
    let mut by_model: BTreeMap<String, SyncCandidate> = BTreeMap::new();
    for entry in data {
        let Some(model_id) = entry
            .get("model_name")
            .and_then(|name| name.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let quota_type = entry.get("quota_type").and_then(|value| value.as_i64());
        let candidate = match quota_type {
            Some(0) => {
                let Some(model_ratio) = json_decimal(entry.get("model_ratio")) else {
                    continue;
                };
                let completion_ratio =
                    json_decimal(entry.get("completion_ratio")).unwrap_or(Decimal::ONE);
                if model_ratio.is_sign_negative() || completion_ratio.is_sign_negative() {
                    continue;
                }
                // MP-Y12a: new-api reports unknown models with the default
                // placeholder ratio 37.5 (75 USD/1M) and completion ratio 1;
                // these prices are untrusted and skipped.
                if model_ratio == Decimal::new(375, 1) && completion_ratio == Decimal::ONE {
                    continue;
                }
                let input = model_ratio * Decimal::TWO;
                let output = input * completion_ratio;
                SyncCandidate {
                    model_id: model_id.to_string(),
                    billing_mode: "per_token".to_string(),
                    input_usd_per_1m: Some(trunc9(input)),
                    output_usd_per_1m: Some(trunc9(output)),
                    cache_read_usd_per_1m: None,
                    cache_write_usd_per_1m: None,
                    reasoning_usd_per_1m: None,
                    per_request_usd: None,
                    billing_expr: None,
                    raw_json: entry.clone(),
                }
            }
            Some(1) => {
                let Some(price) = json_decimal(entry.get("model_price")) else {
                    continue;
                };
                if price.is_sign_negative() {
                    continue;
                }
                SyncCandidate {
                    model_id: model_id.to_string(),
                    billing_mode: "per_request".to_string(),
                    input_usd_per_1m: None,
                    output_usd_per_1m: None,
                    cache_read_usd_per_1m: None,
                    cache_write_usd_per_1m: None,
                    reasoning_usd_per_1m: None,
                    per_request_usd: Some(trunc9(price)),
                    billing_expr: None,
                    raw_json: entry.clone(),
                }
            }
            _ => continue,
        };
        by_model.insert(candidate.model_id.clone(), candidate);
    }
    Ok(by_model.into_values().collect())
}

fn merge_field<T: Clone + PartialEq>(
    field: &str,
    locked: &[String],
    stored: &Option<T>,
    incoming: &Option<T>,
    changed: &mut Vec<String>,
) -> Option<T> {
    if locked.iter().any(|lock| lock == field) {
        return stored.clone();
    }
    if stored != incoming {
        changed.push(field.to_string());
    }
    incoming.clone()
}

/// MP-Y13..MP-Y15: computes the apply plan for one snapshot. `existing` is
/// the full stored row set; ownership, locks, and models_dev deletes are
/// resolved here so apply only needs set-based writes.
pub fn compute_sync_plan(
    source: SyncSource,
    existing: &[ModelPriceRecord],
    candidates: Vec<SyncCandidate>,
) -> SyncPlan {
    let existing_by_id: HashMap<&str, &ModelPriceRecord> = existing
        .iter()
        .map(|row| (row.model_id.as_str(), row))
        .collect();
    let now = Utc::now();
    let mut plan = SyncPlan {
        inserted: 0,
        updated: 0,
        skipped: 0,
        deleted: 0,
        changes: Vec::new(),
        truncated: false,
        writes: Vec::new(),
        delete_ids: Vec::new(),
    };
    let push_change = |plan: &mut SyncPlan, change: SyncChange| {
        if plan.changes.len() < MAX_CHANGE_ENTRIES {
            plan.changes.push(change);
        } else {
            plan.truncated = true;
        }
    };

    let mut candidate_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for candidate in candidates {
        candidate_ids.insert(candidate.model_id.clone());
        match existing_by_id.get(candidate.model_id.as_str()) {
            None => {
                let mut fields: Vec<String> = vec!["billing_mode".to_string()];
                for (name, value) in [
                    ("input_usd_per_1m", &candidate.input_usd_per_1m),
                    ("output_usd_per_1m", &candidate.output_usd_per_1m),
                    ("cache_read_usd_per_1m", &candidate.cache_read_usd_per_1m),
                    ("cache_write_usd_per_1m", &candidate.cache_write_usd_per_1m),
                    ("reasoning_usd_per_1m", &candidate.reasoning_usd_per_1m),
                    ("per_request_usd", &candidate.per_request_usd),
                ] {
                    if value.is_some() {
                        fields.push(name.to_string());
                    }
                }
                if candidate.billing_expr.is_some() {
                    fields.push("billing_expr".to_string());
                }
                plan.inserted += 1;
                push_change(
                    &mut plan,
                    SyncChange {
                        model_id: candidate.model_id.clone(),
                        kind: "insert",
                        fields,
                    },
                );
                plan.writes.push(ModelPriceRecord {
                    model_id: candidate.model_id,
                    billing_mode: candidate.billing_mode,
                    input_usd_per_1m: candidate.input_usd_per_1m,
                    output_usd_per_1m: candidate.output_usd_per_1m,
                    cache_read_usd_per_1m: candidate.cache_read_usd_per_1m,
                    cache_write_usd_per_1m: candidate.cache_write_usd_per_1m,
                    cache_write_1h_usd_per_1m: None,
                    reasoning_usd_per_1m: candidate.reasoning_usd_per_1m,
                    per_request_usd: candidate.per_request_usd,
                    billing_expr: candidate.billing_expr,
                    source: source.as_str().to_string(),
                    locked_fields: Vec::new(),
                    raw_json: candidate.raw_json,
                    enabled: true,
                    updated_at: now,
                });
            }
            Some(stored) if stored.source != source.as_str() => {
                // MP-Y13: manual rows and rows owned by another source are
                // never modified by this run.
                plan.skipped += 1;
                push_change(
                    &mut plan,
                    SyncChange {
                        model_id: candidate.model_id.clone(),
                        kind: "skip",
                        fields: Vec::new(),
                    },
                );
            }
            Some(stored) => {
                let locked = &stored.locked_fields;
                let mut changed: Vec<String> = Vec::new();
                let billing_mode = if locked.iter().any(|lock| lock == "billing_mode") {
                    stored.billing_mode.clone()
                } else {
                    if stored.billing_mode != candidate.billing_mode {
                        changed.push("billing_mode".to_string());
                    }
                    candidate.billing_mode.clone()
                };
                let merged = ModelPriceRecord {
                    model_id: candidate.model_id.clone(),
                    billing_mode,
                    input_usd_per_1m: merge_field(
                        "input_usd_per_1m",
                        locked,
                        &stored.input_usd_per_1m,
                        &candidate.input_usd_per_1m,
                        &mut changed,
                    ),
                    output_usd_per_1m: merge_field(
                        "output_usd_per_1m",
                        locked,
                        &stored.output_usd_per_1m,
                        &candidate.output_usd_per_1m,
                        &mut changed,
                    ),
                    cache_read_usd_per_1m: merge_field(
                        "cache_read_usd_per_1m",
                        locked,
                        &stored.cache_read_usd_per_1m,
                        &candidate.cache_read_usd_per_1m,
                        &mut changed,
                    ),
                    cache_write_usd_per_1m: merge_field(
                        "cache_write_usd_per_1m",
                        locked,
                        &stored.cache_write_usd_per_1m,
                        &candidate.cache_write_usd_per_1m,
                        &mut changed,
                    ),
                    // No sync source publishes a 1h cache-write price; the
                    // stored value is always kept.
                    cache_write_1h_usd_per_1m: stored.cache_write_1h_usd_per_1m.clone(),
                    reasoning_usd_per_1m: merge_field(
                        "reasoning_usd_per_1m",
                        locked,
                        &stored.reasoning_usd_per_1m,
                        &candidate.reasoning_usd_per_1m,
                        &mut changed,
                    ),
                    per_request_usd: merge_field(
                        "per_request_usd",
                        locked,
                        &stored.per_request_usd,
                        &candidate.per_request_usd,
                        &mut changed,
                    ),
                    billing_expr: merge_field(
                        "billing_expr",
                        locked,
                        &stored.billing_expr,
                        &candidate.billing_expr,
                        &mut changed,
                    ),
                    source: source.as_str().to_string(),
                    locked_fields: stored.locked_fields.clone(),
                    raw_json: candidate.raw_json,
                    enabled: stored.enabled,
                    updated_at: now,
                };
                if changed.is_empty() {
                    // MP-Y14: raw_json and updated_at still refresh, but an
                    // unchanged row is not counted as an update.
                    plan.skipped += 1;
                } else {
                    plan.updated += 1;
                    push_change(
                        &mut plan,
                        SyncChange {
                            model_id: candidate.model_id.clone(),
                            kind: "update",
                            fields: changed,
                        },
                    );
                }
                plan.writes.push(merged);
            }
        }
    }

    if source == SyncSource::ModelsDev {
        for row in existing {
            if row.source == "models_dev" && !candidate_ids.contains(&row.model_id) {
                plan.deleted += 1;
                push_change(
                    &mut plan,
                    SyncChange {
                        model_id: row.model_id.clone(),
                        kind: "delete",
                        fields: Vec::new(),
                    },
                );
                plan.delete_ids.push(row.model_id.clone());
            }
        }
    }

    plan
}

/// MP-A6 preview body.
pub fn preview_response(source: SyncSource, plan: &SyncPlan) -> Value {
    let mut body = json!({
        "source": source.as_str(),
        "insert": plan.inserted,
        "update": plan.updated,
        "skip": plan.skipped,
        "delete": plan.deleted,
        "changes": plan.changes,
    });
    if plan.truncated {
        body["truncated"] = json!(true);
    }
    body
}

/// MP-D8: run detail JSON, truncated below the byte bound.
pub fn run_detail_json(plan: &SyncPlan) -> Value {
    let mut detail = json!({ "changes": plan.changes });
    if plan.truncated {
        detail["truncated"] = json!(true);
    }
    let serialized = detail.to_string();
    if serialized.len() > MAX_DETAIL_BYTES {
        detail = json!({ "changes": [], "truncated": true });
    }
    detail
}

/// MP-A7/MP-Y16: performs one apply run end to end and returns the finalized
/// run row. The run row is inserted before the fetch so failures audit too.
pub async fn apply_sync_run(
    http: &reqwest::Client,
    store: &ModelPriceStore,
    registry: &crate::model_registry_store::ModelRegistryStore,
    source: SyncSource,
    new_api_config: (&str, &str),
) -> Result<PriceSyncRun, (PriceSyncRun, String)> {
    let _apply_guard = apply_lock().lock().await;
    let run = store
        .insert_sync_run(source.as_str())
        .await
        .map_err(|error| {
            (
                PriceSyncRun {
                    id: String::new(),
                    source: source.as_str().to_string(),
                    status: "failed".to_string(),
                    started_at: Utc::now(),
                    finished_at: Some(Utc::now()),
                    inserted: 0,
                    updated: 0,
                    skipped: 0,
                    deleted: 0,
                    error: Some(error.clone()),
                    detail_json: json!({}),
                },
                error,
            )
        })?;

    let outcome: Result<PriceSyncRun, String> = async {
        let snapshot = fetch_source_snapshot(http, source, new_api_config).await?;
        let existing = store.list().await?;
        let plan = compute_sync_plan(source, &existing, snapshot.candidates);
        store.bulk_upsert_synced(&plan.writes).await?;
        if !plan.delete_ids.is_empty() {
            store
                .delete_by_ids_with_source(&plan.delete_ids, source.as_str())
                .await?;
        }
        // MP-Y10: a models_dev price sync also refreshes metadata rows.
        if let Some(root) = &snapshot.models_dev_root {
            registry.apply_models_dev_metadata(root).await?;
        }
        store
            .finalize_sync_run(
                &run.id,
                "success",
                plan.inserted,
                plan.updated,
                plan.skipped,
                plan.deleted,
                None,
                &run_detail_json(&plan),
            )
            .await
    }
    .await;

    match outcome {
        Ok(finalized) => Ok(finalized),
        Err(error) => {
            let finalized = store
                .finalize_sync_run(&run.id, "failed", 0, 0, 0, 0, Some(&error), &json!({}))
                .await
                .unwrap_or(run);
            Err((finalized, error))
        }
    }
}

fn auto_sync_is_due(
    enabled: bool,
    was_enabled: bool,
    elapsed_since_cycle_start: Option<Duration>,
) -> bool {
    enabled
        && (!was_enabled
            || elapsed_since_cycle_start.is_none_or(|elapsed| elapsed >= AUTO_SYNC_INTERVAL))
}

async fn run_auto_sync_cycle(
    http: &reqwest::Client,
    store: &ModelPriceStore,
    registry: &crate::model_registry_store::ModelRegistryStore,
) {
    let models_dev = apply_sync_run(http, store, registry, SyncSource::ModelsDev, ("", "")).await;
    match models_dev {
        Ok(run) => {
            tracing::info!(
                source = "models_dev",
                inserted = run.inserted,
                updated = run.updated,
                skipped = run.skipped,
                deleted = run.deleted,
                "automatic price sync completed"
            );
        }
        Err((_, error)) => {
            tracing::warn!(
                source = "models_dev",
                error = %error,
                "automatic price sync failed; skipping OpenRouter"
            );
            return;
        }
    }

    match apply_sync_run(http, store, registry, SyncSource::OpenRouter, ("", "")).await {
        Ok(run) => {
            tracing::info!(
                source = "openrouter",
                inserted = run.inserted,
                updated = run.updated,
                skipped = run.skipped,
                deleted = run.deleted,
                "automatic price sync completed"
            );
        }
        Err((_, error)) => {
            tracing::warn!(
                source = "openrouter",
                error = %error,
                "automatic price sync failed"
            );
        }
    }
}

/// MP-Y19..MP-Y24: starts the primary-only automatic sync scheduler.
pub fn spawn_auto_sync_scheduler(
    http: reqwest::Client,
    store: ModelPriceStore,
    registry: crate::model_registry_store::ModelRegistryStore,
    settings: crate::settings::SettingsStore,
    shutdown: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut was_enabled = false;
        let mut last_cycle_started: Option<tokio::time::Instant> = None;
        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            match settings.get_all().await {
                Ok(current) => {
                    let elapsed = last_cycle_started.map(|started| started.elapsed());
                    let due =
                        auto_sync_is_due(current.price_sync_auto_enabled, was_enabled, elapsed);
                    was_enabled = current.price_sync_auto_enabled;
                    if due {
                        last_cycle_started = Some(tokio::time::Instant::now());
                        run_auto_sync_cycle(&http, &store, &registry).await;
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "automatic price sync could not read settings");
                }
            }
            tokio::time::sleep(AUTO_SYNC_SETTING_POLL_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_row(model_id: &str, source: &str, input: &str) -> ModelPriceRecord {
        ModelPriceRecord {
            model_id: model_id.to_string(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: Some(input.to_string()),
            output_usd_per_1m: Some("10".to_string()),
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            cache_write_1h_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            source: source.to_string(),
            locked_fields: Vec::new(),
            raw_json: json!({}),
            enabled: true,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn models_dev_mapping_selects_official_provider_and_keeps_exact_strings() {
        let root = json!({
            "openai": { "models": { "gpt-4o": {
                "cost": { "input": 2.5, "output": 10, "cache_read": 1.25 },
                "limit": { "context": 128000 }
            } } },
            "reseller": { "models": { "gpt-4o": {
                "cost": { "input": 99.75, "output": 200 }
            } } }
        });
        let candidates = map_models_dev(&root).unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.model_id, "gpt-4o");
        // MP-Y7.1: the official openai variant wins over the pricier reseller.
        assert_eq!(candidate.input_usd_per_1m.as_deref(), Some("2.5"));
        assert_eq!(candidate.output_usd_per_1m.as_deref(), Some("10"));
        assert_eq!(candidate.cache_read_usd_per_1m.as_deref(), Some("1.25"));
        // MP-Y6: every variant lands in raw_json.providers.
        assert!(candidate.raw_json["providers"]["openai"].is_object());
        assert!(candidate.raw_json["providers"]["reseller"].is_object());
    }

    #[test]
    fn models_dev_mapping_converts_gpt_and_claude_context_tiers() {
        let root = json!({
            "openai": { "models": { "gpt-5.6": {
                "cost": {
                    "input": 4, "output": 20, "cache_read": 0.4, "cache_write": 5,
                    "tiers": [{
                        "input": 8, "output": 30, "cache_read": 0.8, "cache_write": 10,
                        "tier": { "type": "context", "size": 272000 }
                    }]
                }
            } } },
            "anthropic": { "models": { "claude-sonnet-4-6": {
                "cost": {
                    "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75,
                    "tiers": [{
                        "input": 6, "output": 22.5, "cache_read": 0.6, "cache_write": 7.5,
                        "tier": { "type": "context", "size": 200000 }
                    }]
                }
            } } }
        });

        let candidates = map_models_dev(&root).unwrap();
        for (model_id, bound, base_input, tier_input) in [
            ("gpt-5.6", 272000, "4", "8"),
            ("claude-sonnet-4-6", 200000, "3", "6"),
        ] {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.model_id == model_id)
                .unwrap();
            assert_eq!(candidate.billing_mode, "tiered_expr");
            let tiers = candidate.billing_expr.as_ref().unwrap()["tiers"]
                .as_array()
                .unwrap();
            assert_eq!(tiers.len(), 2);
            assert_eq!(tiers[0]["when_input_tokens_lte"], json!(bound));
            assert_eq!(tiers[0]["input_usd_per_1m"], json!(base_input));
            assert_eq!(tiers[1]["input_usd_per_1m"], json!(tier_input));
            assert!(tiers[1].get("when_input_tokens_lte").is_none());
        }

        let plan = compute_sync_plan(SyncSource::ModelsDev, &[], candidates);
        assert!(
            plan.writes
                .iter()
                .all(|row| { row.billing_mode == "tiered_expr" && row.billing_expr.is_some() })
        );
    }

    #[test]
    fn models_dev_mapping_falls_back_to_highest_positive_input() {
        let root = json!({
            "cheap": { "models": { "custom-model": { "cost": { "input": 1, "output": 2 } } } },
            "pricey": { "models": { "custom-model": { "cost": { "input": 3, "output": 6 } } } },
            "free": { "models": { "custom-model": { "cost": { "input": 0 } } } }
        });
        let candidates = map_models_dev(&root).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].input_usd_per_1m.as_deref(), Some("3"));
    }

    #[test]
    fn models_dev_mapping_skips_thinking_auto_and_unpriced() {
        let root = json!({
            "p": { "models": {
                "auto": { "cost": { "input": 1 } },
                "some-model-thinking": { "cost": { "input": 1 } },
                "free-model": { "cost": { "input": 0 } },
                "kept-model": { "cost": { "input": 1 } }
            } }
        });
        let candidates = map_models_dev(&root).unwrap();
        let ids: Vec<&str> = candidates.iter().map(|c| c.model_id.as_str()).collect();
        assert_eq!(ids, vec!["kept-model"]);
    }

    #[test]
    fn openrouter_mapping_scales_per_token_prices_exactly() {
        let root = json!({ "data": [
            { "id": "openai/gpt-4o", "pricing": {
                "prompt": "0.0000025", "completion": "0.00001",
                "input_cache_read": "0.00000125", "input_cache_write": "0.000003125"
            } },
            { "id": "vendor/free-model", "pricing": { "prompt": "0", "completion": "0" } }
        ] });
        let candidates = map_openrouter(&root).unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.model_id, "gpt-4o");
        assert_eq!(candidate.input_usd_per_1m.as_deref(), Some("2.5"));
        assert_eq!(candidate.output_usd_per_1m.as_deref(), Some("10"));
        assert_eq!(candidate.cache_read_usd_per_1m.as_deref(), Some("1.25"));
        assert_eq!(candidate.cache_write_usd_per_1m.as_deref(), Some("3.125"));
    }

    #[test]
    fn openrouter_duplicate_canonical_ids_keep_highest_prompt_price() {
        let root = json!({ "data": [
            { "id": "openai/shared-model", "pricing": { "prompt": "0.000001", "completion": "0.000002" } },
            { "id": "azure/shared-model", "pricing": { "prompt": "0.000004", "completion": "0.000008" } }
        ] });
        let candidates = map_openrouter(&root).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].input_usd_per_1m.as_deref(), Some("4"));
    }

    #[test]
    fn new_api_mapping_converts_ratio_and_fixed_price() {
        let root = json!({ "data": [
            { "model_name": "ratio-model", "quota_type": 0,
              "model_ratio": 1.25, "completion_ratio": 4 },
            { "model_name": "fixed-model", "quota_type": 1, "model_price": 0.05 },
            { "model_name": "placeholder-model", "quota_type": 0,
              "model_ratio": 37.5, "completion_ratio": 1 }
        ] });
        let candidates = map_new_api(&root).unwrap();
        assert_eq!(candidates.len(), 2);
        let ratio = candidates
            .iter()
            .find(|c| c.model_id == "ratio-model")
            .unwrap();
        // ratio 1 = USD 2 per 1M: 1.25 * 2 = 2.5; output 2.5 * 4 = 10.
        assert_eq!(ratio.billing_mode, "per_token");
        assert_eq!(ratio.input_usd_per_1m.as_deref(), Some("2.5"));
        assert_eq!(ratio.output_usd_per_1m.as_deref(), Some("10"));
        let fixed = candidates
            .iter()
            .find(|c| c.model_id == "fixed-model")
            .unwrap();
        assert_eq!(fixed.billing_mode, "per_request");
        assert_eq!(fixed.per_request_usd.as_deref(), Some("0.05"));
    }

    #[test]
    fn plan_respects_source_ownership_and_locks() {
        let existing = vec![
            stored_row("manual-model", "manual", "5"),
            stored_row("other-source-model", "openrouter", "5"),
            {
                let mut row = stored_row("locked-model", "models_dev", "5");
                row.locked_fields = vec!["input_usd_per_1m".to_string()];
                row
            },
            stored_row("stale-model", "models_dev", "5"),
        ];
        let candidates = vec![
            SyncCandidate {
                model_id: "manual-model".to_string(),
                billing_mode: "per_token".to_string(),
                input_usd_per_1m: Some("1".to_string()),
                output_usd_per_1m: Some("2".to_string()),
                cache_read_usd_per_1m: None,
                cache_write_usd_per_1m: None,
                reasoning_usd_per_1m: None,
                per_request_usd: None,
                billing_expr: None,
                raw_json: json!({}),
            },
            SyncCandidate {
                model_id: "locked-model".to_string(),
                billing_mode: "per_token".to_string(),
                input_usd_per_1m: Some("9".to_string()),
                output_usd_per_1m: Some("20".to_string()),
                cache_read_usd_per_1m: None,
                cache_write_usd_per_1m: None,
                reasoning_usd_per_1m: None,
                per_request_usd: None,
                billing_expr: None,
                raw_json: json!({ "fresh": true }),
            },
            SyncCandidate {
                model_id: "new-model".to_string(),
                billing_mode: "per_token".to_string(),
                input_usd_per_1m: Some("3".to_string()),
                output_usd_per_1m: Some("6".to_string()),
                cache_read_usd_per_1m: None,
                cache_write_usd_per_1m: None,
                reasoning_usd_per_1m: None,
                per_request_usd: None,
                billing_expr: None,
                raw_json: json!({}),
            },
        ];
        let plan = compute_sync_plan(SyncSource::ModelsDev, &existing, candidates);
        // manual-model skipped (ownership), locked-model updated but keeps
        // the locked input price, new-model inserted, stale-model deleted.
        assert_eq!(plan.inserted, 1);
        assert_eq!(plan.updated, 1);
        assert_eq!(plan.skipped, 1);
        assert_eq!(plan.deleted, 1);
        assert_eq!(plan.delete_ids, vec!["stale-model".to_string()]);
        let locked = plan
            .writes
            .iter()
            .find(|row| row.model_id == "locked-model")
            .unwrap();
        assert_eq!(locked.input_usd_per_1m.as_deref(), Some("5"));
        assert_eq!(locked.output_usd_per_1m.as_deref(), Some("20"));
        assert_eq!(locked.raw_json, json!({ "fresh": true }));
        assert!(!plan.writes.iter().any(|row| row.model_id == "manual-model"));
    }

    #[test]
    fn plan_counts_unchanged_owned_rows_as_skip_but_still_writes_raw_refresh() {
        let existing = vec![{
            let mut row = stored_row("same-model", "openrouter", "5");
            row.output_usd_per_1m = Some("10".to_string());
            row
        }];
        let candidates = vec![SyncCandidate {
            model_id: "same-model".to_string(),
            billing_mode: "per_token".to_string(),
            input_usd_per_1m: Some("5".to_string()),
            output_usd_per_1m: Some("10".to_string()),
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            reasoning_usd_per_1m: None,
            per_request_usd: None,
            billing_expr: None,
            raw_json: json!({ "refreshed": true }),
        }];
        let plan = compute_sync_plan(SyncSource::OpenRouter, &existing, candidates);
        assert_eq!(plan.updated, 0);
        assert_eq!(plan.skipped, 1);
        assert_eq!(plan.writes.len(), 1);
        assert_eq!(plan.writes[0].raw_json, json!({ "refreshed": true }));
    }

    #[test]
    fn automatic_sync_runs_on_enable_and_after_24_hours() {
        assert!(!auto_sync_is_due(false, false, None));
        assert!(auto_sync_is_due(true, false, None));
        assert!(!auto_sync_is_due(
            true,
            true,
            Some(Duration::from_secs(24 * 60 * 60 - 1))
        ));
        assert!(auto_sync_is_due(
            true,
            true,
            Some(Duration::from_secs(24 * 60 * 60))
        ));
    }
}

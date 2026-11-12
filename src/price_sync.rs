//! External price sync engine (`model-pricing.spec.md` §9).
//!
//! Three sources map upstream price catalogs into `model_prices` rows:
//! `models_dev`, `openrouter`, and `new_api`. Preview computes the diff
//! without writes (MP-A6); apply performs the ownership-scoped upsert with
//! field locks and an audit run row (MP-Y13..MP-Y16).

mod mapping;
mod plan;
pub use mapping::{map_models_dev, map_new_api, map_openrouter};
pub use plan::compute_sync_plan;

use crate::model_price_store::{
    ModelPriceRecord, ModelPriceStore, PriceSyncRun, is_service_tier_key, validate_billing_expr,
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

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SyncChange {
    pub model_id: String,
    pub kind: &'static str,
    pub fields: Vec<String>,
    /// MP-A6: stored values of the changed fields (update, delete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<serde_json::Map<String, Value>>,
    /// MP-A6: values written by apply (insert, update).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<serde_json::Map<String, Value>>,
}

/// The computed apply plan for one source snapshot against the stored rows.
pub struct SyncPlan {
    pub inserted: i32,
    pub updated: i32,
    pub skipped: i32,
    pub deleted: i32,
    /// Every change, untruncated; serializers apply MP-A6/MP-D8 bounds.
    pub changes: Vec<SyncChange>,
    /// Fully merged rows to write (inserts, updates, and owned rows whose
    /// raw_json refreshes without a counted change, per MP-Y14).
    pub writes: Vec<ModelPriceRecord>,
    /// `models_dev` rows absent from the snapshot (MP-Y15).
    pub delete_ids: Vec<String>,
}

/// Fetches `https://models.dev/api.json?type=all` and returns the parsed root.
/// `type=all` is required: the default document omits `type=decision` models,
/// including Jev (MP-Y1a). Errors carry a `fetch_failed:`/`parse_failed:` prefix
/// for HTTP mapping.
pub async fn fetch_models_dev_root(http: &reqwest::Client) -> Result<Value, String> {
    fetch_json(http, "https://models.dev/api.json?type=all", None).await
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

/// MP-A6: at most `MAX_CHANGE_ENTRIES` changes, ordered delete, update,
/// insert, skip so reviewable changes survive truncation.
fn bounded_changes(plan: &SyncPlan) -> (Vec<&SyncChange>, bool) {
    let rank = |kind: &str| match kind {
        "delete" => 0,
        "update" => 1,
        "insert" => 2,
        _ => 3,
    };
    let mut changes: Vec<&SyncChange> = plan.changes.iter().collect();
    changes.sort_by_key(|change| rank(change.kind));
    let truncated = changes.len() > MAX_CHANGE_ENTRIES;
    changes.truncate(MAX_CHANGE_ENTRIES);
    (changes, truncated)
}

/// MP-A6 preview body. `preview_id` identifies the server-side snapshot that
/// MP-A7 applies.
pub fn preview_response(
    source: SyncSource,
    plan: &SyncPlan,
    preview_id: &str,
    expires_at: chrono::DateTime<Utc>,
) -> Value {
    let (changes, truncated) = bounded_changes(plan);
    let mut body = json!({
        "source": source.as_str(),
        "preview_id": preview_id,
        "expires_at": expires_at.to_rfc3339(),
        "insert": plan.inserted,
        "update": plan.updated,
        "skip": plan.skipped,
        "delete": plan.deleted,
        "changes": changes,
    });
    if truncated {
        body["truncated"] = json!(true);
    }
    body
}

/// MP-D8: run detail JSON (field names only), truncated below the byte bound.
pub fn run_detail_json(plan: &SyncPlan) -> Value {
    let (changes, truncated) = bounded_changes(plan);
    let changes: Vec<Value> = changes
        .into_iter()
        .map(|change| {
            json!({ "model_id": change.model_id, "kind": change.kind, "fields": change.fields })
        })
        .collect();
    let mut detail = json!({ "changes": changes });
    if truncated {
        detail["truncated"] = json!(true);
    }
    let serialized = detail.to_string();
    if serialized.len() > MAX_DETAIL_BYTES {
        detail = json!({ "changes": [], "truncated": true });
    }
    detail
}

/// MP-A6a: how long a preview snapshot stays applicable.
pub const PREVIEW_TTL: Duration = Duration::from_secs(10 * 60);
/// Snapshots hold whole source documents; keep only the newest few.
const MAX_STORED_PREVIEWS: usize = 4;

/// A fetched snapshot plus the exact change list shown by MP-A6.
pub struct StoredPreview {
    pub source: SyncSource,
    snapshot: SourceSnapshot,
    changes: Vec<SyncChange>,
    created: std::time::Instant,
}

static PREVIEWS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, StoredPreview>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn previews() -> &'static std::sync::Mutex<HashMap<String, StoredPreview>> {
    &PREVIEWS
}

/// MP-A6: computes the plan for a fetched snapshot, stores the snapshot under
/// a fresh id, and returns the preview body.
pub fn store_preview(
    source: SyncSource,
    existing: &[ModelPriceRecord],
    snapshot: SourceSnapshot,
) -> Value {
    let plan = compute_sync_plan(source, existing, snapshot.candidates.clone());
    let preview_id = uuid::Uuid::new_v4().to_string();
    let expires_at = Utc::now()
        + chrono::Duration::from_std(PREVIEW_TTL).unwrap_or(chrono::Duration::minutes(10));
    let body = preview_response(source, &plan, &preview_id, expires_at);
    let mut stored = previews()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    stored.retain(|_, preview| preview.created.elapsed() < PREVIEW_TTL);
    while stored.len() >= MAX_STORED_PREVIEWS {
        let Some(oldest) = stored
            .iter()
            .min_by_key(|(_, preview)| preview.created)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        stored.remove(&oldest);
    }
    stored.insert(
        preview_id,
        StoredPreview {
            source,
            snapshot,
            changes: plan.changes,
            created: std::time::Instant::now(),
        },
    );
    body
}

/// MP-A7: removes and returns an unexpired preview of `source`.
pub fn take_preview(preview_id: &str, source: SyncSource) -> Option<StoredPreview> {
    let mut stored = previews()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let preview = stored.remove(preview_id)?;
    (preview.source == source && preview.created.elapsed() < PREVIEW_TTL).then_some(preview)
}

/// MP-A7 error for a preview whose change list no longer matches the
/// stored rows.
pub const PREVIEW_STALE_ERROR: &str = "preview_stale";

/// MP-A7: applies a stored preview snapshot. The plan is recomputed against
/// the current rows under the apply lock; when it differs from the previewed
/// change list, nothing is written and no run row is created.
pub async fn apply_previewed_sync_run(
    store: &ModelPriceStore,
    registry: &crate::model_registry_store::ModelRegistryStore,
    preview: StoredPreview,
) -> Result<PriceSyncRun, String> {
    let _apply_guard = apply_lock().lock().await;
    let existing = store.list().await?;
    let plan = compute_sync_plan(preview.source, &existing, preview.snapshot.candidates);
    if plan.changes != preview.changes {
        return Err(PREVIEW_STALE_ERROR.to_string());
    }
    let run = store.insert_sync_run(preview.source.as_str()).await?;
    let outcome = write_plan(
        store,
        registry,
        preview.source,
        &run.id,
        &plan,
        preview.snapshot.models_dev_root.as_ref(),
    )
    .await;
    match outcome {
        Ok(finalized) => Ok(finalized),
        Err(error) => {
            let _ = store
                .finalize_sync_run(
                    &run.id,
                    crate::model_price_store::SyncRunOutcome {
                        status: "failed",
                        inserted: 0,
                        updated: 0,
                        skipped: 0,
                        deleted: 0,
                        error: Some(&error),
                        detail_json: &json!({}),
                    },
                )
                .await;
            Err(error)
        }
    }
}

async fn write_plan(
    store: &ModelPriceStore,
    registry: &crate::model_registry_store::ModelRegistryStore,
    source: SyncSource,
    run_id: &str,
    plan: &SyncPlan,
    models_dev_root: Option<&Value>,
) -> Result<PriceSyncRun, String> {
    store.bulk_upsert_synced(&plan.writes).await?;
    if !plan.delete_ids.is_empty() {
        store
            .delete_by_ids_with_source(&plan.delete_ids, source.as_str())
            .await?;
    }
    // MP-Y10: a models_dev price sync also refreshes metadata rows.
    if let Some(root) = models_dev_root {
        registry.apply_models_dev_metadata(root).await?;
    }
    store
        .finalize_sync_run(
            run_id,
            crate::model_price_store::SyncRunOutcome {
                status: "success",
                inserted: plan.inserted,
                updated: plan.updated,
                skipped: plan.skipped,
                deleted: plan.deleted,
                error: None,
                detail_json: &run_detail_json(plan),
            },
        )
        .await
}

/// MP-Y16/MP-Y20: fetch-and-apply run used by the automatic scheduler. The run
/// row is inserted before the fetch so failures audit too. Dashboard applies
/// go through `apply_previewed_sync_run` (MP-A7).
pub async fn apply_sync_run(
    http: &reqwest::Client,
    store: &ModelPriceStore,
    registry: &crate::model_registry_store::ModelRegistryStore,
    source: SyncSource,
    new_api_config: (&str, &str),
) -> Result<PriceSyncRun, (Box<PriceSyncRun>, String)> {
    let _apply_guard = apply_lock().lock().await;
    let run = store
        .insert_sync_run(source.as_str())
        .await
        .map_err(|error| {
            (
                Box::new(PriceSyncRun {
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
                }),
                error,
            )
        })?;

    let outcome: Result<PriceSyncRun, String> = async {
        let snapshot = fetch_source_snapshot(http, source, new_api_config).await?;
        let existing = store.list().await?;
        let plan = compute_sync_plan(source, &existing, snapshot.candidates);
        write_plan(
            store,
            registry,
            source,
            &run.id,
            &plan,
            snapshot.models_dev_root.as_ref(),
        )
        .await
    }
    .await;

    match outcome {
        Ok(finalized) => Ok(finalized),
        Err(error) => {
            let finalized = store
                .finalize_sync_run(
                    &run.id,
                    crate::model_price_store::SyncRunOutcome {
                        status: "failed",
                        inserted: 0,
                        updated: 0,
                        skipped: 0,
                        deleted: 0,
                        error: Some(&error),
                        detail_json: &json!({}),
                    },
                )
                .await
                .unwrap_or(run);
            Err((Box::new(finalized), error))
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

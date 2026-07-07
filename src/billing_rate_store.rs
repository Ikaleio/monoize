use crate::db::DbPool;
use crate::settings::PricingProfilePattern;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

const BILLING_RATE_CATALOG: &str = include_str!("billing-rates.catalog.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbBillingRateRecord {
    pub id: String,
    pub source: String,
    pub pricing_profile: String,
    pub model_pattern: Option<String>,
    pub provider_type: Option<String>,
    pub rate_kind: String,
    pub usage_class: String,
    pub unit: String,
    pub unit_price_nano_usd: String,
    pub context_tier: Option<String>,
    pub service_tier: Option<String>,
    pub modality: Option<String>,
    pub cache_ttl: Option<String>,
    pub match_json: Value,
    pub priority: i32,
    pub enabled: bool,
    pub raw_json: Value,
    pub updated_at: DateTime<Utc>,
}

impl DbBillingRateRecord {
    pub fn unit_price_nano(&self) -> Result<i128, String> {
        self.unit_price_nano_usd
            .parse::<i128>()
            .map_err(|_| format!("invalid unit_price_nano_usd for {}", self.id))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertBillingRateInput {
    pub source: Option<String>,
    pub pricing_profile: Option<String>,
    pub model_pattern: Option<Option<String>>,
    pub provider_type: Option<Option<String>>,
    pub rate_kind: Option<String>,
    pub usage_class: Option<String>,
    pub unit: Option<String>,
    pub unit_price_nano_usd: Option<String>,
    pub context_tier: Option<Option<String>>,
    pub service_tier: Option<Option<String>>,
    pub modality: Option<Option<String>>,
    pub cache_ttl: Option<Option<String>>,
    pub match_json: Option<Value>,
    pub priority: Option<i32>,
    pub enabled: Option<bool>,
    pub raw_json: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingRateSyncResult {
    pub success: bool,
    pub upserted: usize,
    pub skipped: usize,
    pub deleted: u64,
    pub fetched_at: String,
}

#[derive(Debug, Deserialize)]
struct CatalogRoot {
    rates: Vec<CatalogBillingRate>,
}

#[derive(Debug, Deserialize)]
struct CatalogBillingRate {
    id: String,
    pricing_profile: String,
    #[serde(default)]
    model_pattern: Option<String>,
    #[serde(default)]
    provider_type: Option<String>,
    rate_kind: String,
    usage_class: String,
    unit: String,
    unit_price_nano_usd: String,
    #[serde(default)]
    context_tier: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
    #[serde(default)]
    modality: Option<String>,
    #[serde(default)]
    cache_ttl: Option<String>,
    #[serde(default)]
    match_json: Value,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    raw_json: Value,
}

fn default_true() -> bool {
    true
}

#[derive(Clone)]
pub struct BillingRateStore {
    db: DbPool,
}

impl BillingRateStore {
    pub async fn new(db: DbPool) -> Result<Self, String> {
        Ok(Self { db })
    }

    pub async fn list_billing_rates(&self) -> Result<Vec<DbBillingRateRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, source, pricing_profile, model_pattern, provider_type, rate_kind,
                        usage_class, unit, unit_price_nano_usd, context_tier, service_tier,
                        modality, cache_ttl, match_json, priority, enabled, raw_json, updated_at
                 FROM billing_rate_records
                 ORDER BY pricing_profile ASC, priority DESC, id ASC",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(row_to_billing_rate).collect()
    }

    pub async fn list_matching_rates(
        &self,
        pricing_profile: &str,
        provider_type: Option<&str>,
        model: &str,
    ) -> Result<Vec<DbBillingRateRecord>, String> {
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                "SELECT id, source, pricing_profile, model_pattern, provider_type, rate_kind,
                        usage_class, unit, unit_price_nano_usd, context_tier, service_tier,
                        modality, cache_ttl, match_json, priority, enabled, raw_json, updated_at
                 FROM billing_rate_records
                 WHERE enabled = 1
                   AND pricing_profile = $1
                   AND (provider_type IS NULL OR provider_type = $2)
                 ORDER BY priority DESC, id ASC",
                vec![pricing_profile.into(), provider_type.unwrap_or("").into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.iter()
            .map(row_to_billing_rate)
            .filter_map(|result| match result {
                Ok(rate) => {
                    if rate
                        .model_pattern
                        .as_deref()
                        .is_none_or(|pattern| glob_matches(pattern, model))
                    {
                        Some(Ok(rate))
                    } else {
                        None
                    }
                }
                Err(err) => Some(Err(err)),
            })
            .collect()
    }

    pub async fn upsert_billing_rate(
        &self,
        id: &str,
        input: UpsertBillingRateInput,
    ) -> Result<DbBillingRateRecord, String> {
        if id.trim().is_empty() {
            return Err("id must not be empty".to_string());
        }

        let existing = self.get_billing_rate(id).await?;
        let source = input.source.unwrap_or_else(|| "manual".to_string());
        let pricing_profile = input
            .pricing_profile
            .or_else(|| existing.as_ref().map(|r| r.pricing_profile.clone()))
            .ok_or_else(|| "pricing_profile is required".to_string())?;
        let rate_kind = input
            .rate_kind
            .or_else(|| existing.as_ref().map(|r| r.rate_kind.clone()))
            .ok_or_else(|| "rate_kind is required".to_string())?;
        let usage_class = input
            .usage_class
            .or_else(|| existing.as_ref().map(|r| r.usage_class.clone()))
            .ok_or_else(|| "usage_class is required".to_string())?;
        let unit = input
            .unit
            .or_else(|| existing.as_ref().map(|r| r.unit.clone()))
            .ok_or_else(|| "unit is required".to_string())?;
        let unit_price_nano_usd = input
            .unit_price_nano_usd
            .or_else(|| existing.as_ref().map(|r| r.unit_price_nano_usd.clone()))
            .ok_or_else(|| "unit_price_nano_usd is required".to_string())?;
        unit_price_nano_usd
            .parse::<i128>()
            .map_err(|_| "unit_price_nano_usd must be an integer string".to_string())?;

        let model_pattern = input
            .model_pattern
            .unwrap_or_else(|| existing.as_ref().and_then(|r| r.model_pattern.clone()));
        let provider_type = input
            .provider_type
            .unwrap_or_else(|| existing.as_ref().and_then(|r| r.provider_type.clone()));
        let context_tier = input
            .context_tier
            .unwrap_or_else(|| existing.as_ref().and_then(|r| r.context_tier.clone()));
        let service_tier = input
            .service_tier
            .unwrap_or_else(|| existing.as_ref().and_then(|r| r.service_tier.clone()));
        let modality = input
            .modality
            .unwrap_or_else(|| existing.as_ref().and_then(|r| r.modality.clone()));
        let cache_ttl = input
            .cache_ttl
            .unwrap_or_else(|| existing.as_ref().and_then(|r| r.cache_ttl.clone()));
        let match_json = input
            .match_json
            .or_else(|| existing.as_ref().map(|r| r.match_json.clone()))
            .unwrap_or_else(|| serde_json::json!({}));
        let priority = input
            .priority
            .or_else(|| existing.as_ref().map(|r| r.priority))
            .unwrap_or(0);
        let enabled = input
            .enabled
            .or_else(|| existing.as_ref().map(|r| r.enabled))
            .unwrap_or(true);
        let raw_json = input
            .raw_json
            .or_else(|| existing.as_ref().map(|r| r.raw_json.clone()))
            .unwrap_or_else(|| serde_json::json!({}));
        let now = Utc::now().to_rfc3339();

        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "INSERT INTO billing_rate_records
                 (id, source, pricing_profile, model_pattern, provider_type, rate_kind, usage_class,
                  unit, unit_price_nano_usd, context_tier, service_tier, modality, cache_ttl,
                  match_json, priority, enabled, raw_json, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
                 ON CONFLICT(id) DO UPDATE SET
                   source = excluded.source,
                   pricing_profile = excluded.pricing_profile,
                   model_pattern = excluded.model_pattern,
                   provider_type = excluded.provider_type,
                   rate_kind = excluded.rate_kind,
                   usage_class = excluded.usage_class,
                   unit = excluded.unit,
                   unit_price_nano_usd = excluded.unit_price_nano_usd,
                   context_tier = excluded.context_tier,
                   service_tier = excluded.service_tier,
                   modality = excluded.modality,
                   cache_ttl = excluded.cache_ttl,
                   match_json = excluded.match_json,
                   priority = excluded.priority,
                   enabled = excluded.enabled,
                   raw_json = excluded.raw_json,
                   updated_at = excluded.updated_at",
                vec![
                    id.to_string().into(),
                    source.into(),
                    pricing_profile.into(),
                    model_pattern.into(),
                    provider_type.into(),
                    rate_kind.into(),
                    usage_class.into(),
                    unit.into(),
                    unit_price_nano_usd.into(),
                    context_tier.into(),
                    service_tier.into(),
                    modality.into(),
                    cache_ttl.into(),
                    match_json.to_string().into(),
                    priority.into(),
                    (if enabled { 1_i32 } else { 0_i32 }).into(),
                    raw_json.to_string().into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        self.get_billing_rate(id)
            .await?
            .ok_or_else(|| "upsert succeeded but billing rate not found".to_string())
    }

    pub async fn get_billing_rate(&self, id: &str) -> Result<Option<DbBillingRateRecord>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, source, pricing_profile, model_pattern, provider_type, rate_kind,
                        usage_class, unit, unit_price_nano_usd, context_tier, service_tier,
                        modality, cache_ttl, match_json, priority, enabled, raw_json, updated_at
                 FROM billing_rate_records
                 WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        row.as_ref().map(row_to_billing_rate).transpose()
    }

    pub async fn delete_billing_rate(&self, id: &str) -> Result<bool, String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM billing_rate_records WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn sync_catalog(&self) -> Result<BillingRateSyncResult, String> {
        let catalog: CatalogRoot = serde_json::from_str(BILLING_RATE_CATALOG)
            .map_err(|e| format!("catalog_parse_failed: {e}"))?;
        let fetched_at = Utc::now().to_rfc3339();
        let _write_guard = self.db.write().await;
        let txn = _write_guard.begin().await.map_err(|e| e.to_string())?;

        let manual_rows = txn
            .query_all(self.db.stmt(
                "SELECT id FROM billing_rate_records WHERE source = 'manual'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let manual_ids: HashSet<String> = manual_rows
            .iter()
            .filter_map(|row| row.try_get::<String>("", "id").ok())
            .collect();

        let del_result = txn
            .execute(self.db.stmt(
                "DELETE FROM billing_rate_records WHERE source = 'catalog'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let deleted = del_result.rows_affected();

        let mut upserted = 0usize;
        let mut skipped = 0usize;
        for rate in catalog.rates {
            if manual_ids.contains(&rate.id) {
                skipped += 1;
                continue;
            }
            txn.execute(self.db.stmt(
                "INSERT INTO billing_rate_records
                 (id, source, pricing_profile, model_pattern, provider_type, rate_kind, usage_class,
                  unit, unit_price_nano_usd, context_tier, service_tier, modality, cache_ttl,
                  match_json, priority, enabled, raw_json, updated_at)
                 VALUES ($1, 'catalog', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
                vec![
                    rate.id.into(),
                    rate.pricing_profile.into(),
                    rate.model_pattern.into(),
                    rate.provider_type.into(),
                    rate.rate_kind.into(),
                    rate.usage_class.into(),
                    rate.unit.into(),
                    rate.unit_price_nano_usd.into(),
                    rate.context_tier.into(),
                    rate.service_tier.into(),
                    rate.modality.into(),
                    rate.cache_ttl.into(),
                    rate.match_json.to_string().into(),
                    rate.priority.into(),
                    (if rate.enabled { 1_i32 } else { 0_i32 }).into(),
                    rate.raw_json.to_string().into(),
                    fetched_at.clone().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
            upserted += 1;
        }

        txn.commit().await.map_err(|e| e.to_string())?;
        Ok(BillingRateSyncResult {
            success: true,
            upserted,
            skipped,
            deleted,
            fetched_at,
        })
    }
}

pub fn glob_matches(pattern: &str, value: &str) -> bool {
    fn inner(p: &[u8], v: &[u8]) -> bool {
        if p.is_empty() {
            return v.is_empty();
        }
        match p[0] {
            b'*' => inner(&p[1..], v) || (!v.is_empty() && inner(p, &v[1..])),
            b'?' => !v.is_empty() && inner(&p[1..], &v[1..]),
            ch => !v.is_empty() && ch.eq_ignore_ascii_case(&v[0]) && inner(&p[1..], &v[1..]),
        }
    }
    inner(pattern.as_bytes(), value.as_bytes())
}

pub fn select_pricing_profile<'a>(
    patterns: &'a [PricingProfilePattern],
    model: &str,
) -> Option<&'a str> {
    patterns
        .iter()
        .find(|entry| glob_matches(&entry.pattern, model))
        .map(|entry| entry.pricing_profile.as_str())
}

fn row_to_billing_rate(row: &sea_orm::QueryResult) -> Result<DbBillingRateRecord, String> {
    let match_json_raw: String = row.try_get("", "match_json").map_err(|e| e.to_string())?;
    let raw_json_raw: String = row.try_get("", "raw_json").map_err(|e| e.to_string())?;
    let updated_at_raw: String = row.try_get("", "updated_at").map_err(|e| e.to_string())?;
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_raw)
        .map_err(|e| e.to_string())?
        .with_timezone(&Utc);
    let enabled_i: i32 = row.try_get("", "enabled").map_err(|e| e.to_string())?;

    Ok(DbBillingRateRecord {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        source: row.try_get("", "source").map_err(|e| e.to_string())?,
        pricing_profile: row
            .try_get("", "pricing_profile")
            .map_err(|e| e.to_string())?,
        model_pattern: row
            .try_get("", "model_pattern")
            .map_err(|e| e.to_string())?,
        provider_type: row
            .try_get("", "provider_type")
            .map_err(|e| e.to_string())?,
        rate_kind: row.try_get("", "rate_kind").map_err(|e| e.to_string())?,
        usage_class: row.try_get("", "usage_class").map_err(|e| e.to_string())?,
        unit: row.try_get("", "unit").map_err(|e| e.to_string())?,
        unit_price_nano_usd: row
            .try_get("", "unit_price_nano_usd")
            .map_err(|e| e.to_string())?,
        context_tier: row.try_get("", "context_tier").map_err(|e| e.to_string())?,
        service_tier: row.try_get("", "service_tier").map_err(|e| e.to_string())?,
        modality: row.try_get("", "modality").map_err(|e| e.to_string())?,
        cache_ttl: row.try_get("", "cache_ttl").map_err(|e| e.to_string())?,
        match_json: serde_json::from_str(&match_json_raw).unwrap_or_else(|_| serde_json::json!({})),
        priority: row.try_get("", "priority").map_err(|e| e.to_string())?,
        enabled: enabled_i != 0,
        raw_json: serde_json::from_str(&raw_json_raw).unwrap_or_else(|_| serde_json::json!({})),
        updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::{glob_matches, select_pricing_profile};
    use crate::settings::PricingProfilePattern;

    #[test]
    fn glob_matching_is_case_insensitive_and_orderable() {
        assert!(glob_matches("gpt-*", "GPT-5.5"));
        assert!(glob_matches("claude-sonnet-4?", "claude-sonnet-45"));
        assert!(!glob_matches("claude-opus-*", "claude-sonnet-4"));
    }

    #[test]
    fn pricing_profile_selection_uses_ordered_first_match() {
        let patterns = vec![
            PricingProfilePattern {
                pattern: "gpt-*".to_string(),
                pricing_profile: "first".to_string(),
            },
            PricingProfilePattern {
                pattern: "gpt-image-*".to_string(),
                pricing_profile: "second".to_string(),
            },
            PricingProfilePattern {
                pattern: "*".to_string(),
                pricing_profile: "fallback".to_string(),
            },
        ];

        assert_eq!(
            select_pricing_profile(&patterns, "gpt-image-2"),
            Some("first")
        );
        assert_eq!(
            select_pricing_profile(&patterns, "claude-opus-4"),
            Some("fallback")
        );
    }
}

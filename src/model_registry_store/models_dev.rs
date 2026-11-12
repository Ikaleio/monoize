use super::*;
use rust_decimal::Decimal;
use std::str::FromStr;

impl ModelRegistryStore {
    /// Fetch models.dev and upsert metadata rows (MP-Y10). Metadata rows no
    /// longer carry prices; `model_prices` is written only by the price sync
    /// engine (`price_sync`).
    pub async fn sync_from_models_dev(
        &self,
        http: &reqwest::Client,
    ) -> Result<ModelMetadataSyncResult, String> {
        let root = crate::price_sync::fetch_models_dev_root(http).await?;
        self.apply_models_dev_metadata(&root).await
    }

    /// Apply one parsed models.dev snapshot to `model_metadata_records`:
    /// delete non-manual rows, then insert one row per canonical model that
    /// has a strictly positive input cost variant. Manual rows are kept and
    /// counted as skipped.
    pub async fn apply_models_dev_metadata(
        &self,
        root: &Value,
    ) -> Result<ModelMetadataSyncResult, String> {
        let grouped = group_models_dev_variants(root)?;

        let fetched_at = Utc::now().to_rfc3339();
        let _write_guard = self.db.write().await;
        let txn = _write_guard.begin().await.map_err(|e| e.to_string())?;

        let del_result = txn
            .execute(self.db.stmt(
                "DELETE FROM model_metadata_records WHERE source != 'manual'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let deleted = del_result.rows_affected();

        let manual_rows = txn
            .query_all(self.db.stmt(
                "SELECT model_id FROM model_metadata_records WHERE source = 'manual'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let manual_ids = manual_rows
            .into_iter()
            .map(|row| {
                row.try_get::<String>("", "model_id")
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<std::collections::HashSet<_>, _>>()?;

        let (metadata_writes, skipped) = prepare_metadata_writes(&grouped, &manual_ids);
        self.write_models_dev_metadata(&txn, &metadata_writes, &fetched_at)
            .await?;

        txn.commit().await.map_err(|e| e.to_string())?;
        let upserted = metadata_writes.len();
        Ok(ModelMetadataSyncResult {
            success: true,
            upserted,
            skipped,
            deleted,
            fetched_at,
        })
    }

    async fn write_models_dev_metadata(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        metadata_writes: &[ModelsDevMetadataWrite],
        fetched_at: &str,
    ) -> Result<(), String> {
        const METADATA_SYNC_CHUNK_SIZE: usize = 100;
        for chunk in metadata_writes.chunks(METADATA_SYNC_CHUNK_SIZE) {
            let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * 8);
            let mut rows = Vec::with_capacity(chunk.len());
            for row in chunk {
                let start = values.len() + 1;
                values.extend([
                    row.model_name.clone().into(),
                    row.provider_id.clone().into(),
                    row.mode.into(),
                    row.max_input_tokens.into(),
                    row.max_output_tokens.into(),
                    row.max_tokens.into(),
                    row.raw_json.clone().into(),
                    fetched_at.to_string().into(),
                ]);
                let mut placeholders = (start..start + 7)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>();
                placeholders.push("'models_dev'".to_string());
                placeholders.push(format!("${}", start + 7));
                // Placeholder order: 7 data columns, literal source, updated_at.
                rows.push(format!("({})", placeholders.join(", ")));
            }
            txn.execute(self.db.stmt(
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
                    rows.join(", ")
                ),
                values,
            ))
            .await
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    }
}

fn prepare_metadata_writes(
    grouped: &[(String, Vec<SyncProviderVariant>)],
    manual_ids: &std::collections::HashSet<String>,
) -> (Vec<ModelsDevMetadataWrite>, usize) {
    let mut metadata_writes = Vec::new();
    let mut skipped = 0usize;
    for (model_name, variants) in grouped {
        if !has_positive_input_variant(variants) {
            continue;
        }
        if manual_ids.contains(model_name) {
            skipped += 1;
            continue;
        }

        let winner = pick_best_variant(model_name, variants);
        let mode = if variants
            .iter()
            .any(|v| is_embedding_family(v.family.as_deref()))
        {
            "embedding"
        } else {
            "chat"
        };

        let mut providers_map = serde_json::Map::new();
        for v in variants {
            providers_map.insert(v.provider_id.clone(), v.raw.clone());
        }
        metadata_writes.push(ModelsDevMetadataWrite {
            model_name: model_name.clone(),
            provider_id: winner.provider_id.clone(),
            mode,
            max_input_tokens: winner.max_input_tokens,
            max_output_tokens: winner.max_output_tokens,
            max_tokens: winner.max_tokens,
            raw_json: serde_json::json!({ "providers": providers_map }).to_string(),
        });
    }

    (metadata_writes, skipped)
}

/// One models.dev provider variant of a canonical model, with cost values
/// kept as exact USD-per-1M decimal strings (MP-Y5).
pub(crate) struct SyncProviderVariant {
    pub(crate) provider_id: String,
    pub(crate) family: Option<String>,
    pub(crate) cost_input: Option<String>,
    pub(crate) cost_output: Option<String>,
    pub(crate) cost_cache_read: Option<String>,
    pub(crate) cost_cache_write: Option<String>,
    pub(crate) cost_reasoning: Option<String>,
    pub(crate) max_input_tokens: Option<i64>,
    pub(crate) max_output_tokens: Option<i64>,
    pub(crate) max_tokens: Option<i64>,
    pub(crate) raw: Value,
}

struct ModelsDevMetadataWrite {
    model_name: String,
    provider_id: String,
    mode: &'static str,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    max_tokens: Option<i64>,
    raw_json: String,
}

/// MP-Y4: group models.dev variants by canonical model id (NID1), applying
/// the MP-Y9 skip rules. The result is sorted by canonical id for
/// deterministic write order.
pub(crate) fn group_models_dev_variants(
    root: &Value,
) -> Result<Vec<(String, Vec<SyncProviderVariant>)>, String> {
    let providers = root
        .as_object()
        .ok_or_else(|| "parse_failed: root must be object".to_string())?;
    let mut grouped: std::collections::BTreeMap<String, Vec<SyncProviderVariant>> =
        std::collections::BTreeMap::new();
    for (provider_id, provider_val) in providers {
        let provider_obj = match provider_val.as_object() {
            Some(v) => v,
            None => continue,
        };
        let models = match provider_obj.get("models").and_then(|m| m.as_object()) {
            Some(v) => v,
            None => continue,
        };
        for (model_name, model_val) in models {
            let model_obj = match model_val.as_object() {
                Some(v) => v,
                None => continue,
            };
            let cost = model_obj.get("cost").and_then(|c| c.as_object());
            let limit = model_obj.get("limit").and_then(|l| l.as_object());
            let canonical = normalize_model_id(model_name, Some(provider_id));
            if should_ignore_sync_model(&canonical) {
                continue;
            }
            grouped
                .entry(canonical)
                .or_default()
                .push(SyncProviderVariant {
                    provider_id: provider_id.clone(),
                    family: model_obj
                        .get("family")
                        .and_then(|f| f.as_str())
                        .map(|s| s.to_string()),
                    cost_input: cost.and_then(|c| c.get("input")).and_then(usd_cost_string),
                    cost_output: cost.and_then(|c| c.get("output")).and_then(usd_cost_string),
                    cost_cache_read: cost
                        .and_then(|c| c.get("cache_read"))
                        .and_then(usd_cost_string),
                    cost_cache_write: cost
                        .and_then(|c| c.get("cache_write"))
                        .and_then(usd_cost_string),
                    cost_reasoning: cost
                        .and_then(|c| c.get("reasoning"))
                        .and_then(usd_cost_string),
                    max_input_tokens: limit.and_then(|l| l.get("input")).and_then(value_to_i64),
                    max_output_tokens: limit.and_then(|l| l.get("output")).and_then(value_to_i64),
                    max_tokens: limit.and_then(|l| l.get("context")).and_then(value_to_i64),
                    raw: models_dev_variant_for_dashboard(model_val),
                });
        }
    }
    Ok(grouped.into_iter().collect())
}

/// MP-Y8 official family→provider table: a case-insensitive prefix test on
/// the canonical model id, first matching row wins. `o<digit>` is the letter
/// `o` followed by an ASCII digit.
const OFFICIAL_FAMILY_PROVIDERS: &[(&str, &str)] = &[
    ("gpt-", "openai"),
    ("chatgpt-", "openai"),
    ("claude-", "anthropic"),
    ("gemini-", "google"),
    ("gemma-", "google"),
    ("grok-", "xai"),
    ("deepseek-", "deepseek"),
    ("mistral-", "mistral"),
    ("codestral-", "mistral"),
    ("pixtral-", "mistral"),
    ("ministral-", "mistral"),
    ("magistral-", "mistral"),
    ("devstral-", "mistral"),
    ("qwen", "alibaba"),
    ("qwq-", "alibaba"),
    ("qvq-", "alibaba"),
    ("llama-", "llama"),
    ("command-", "cohere"),
    ("kimi-", "moonshotai"),
    ("moonshot-", "moonshotai"),
    ("glm-", "zhipuai"),
    ("minimax-", "minimax"),
    ("step-", "stepfun"),
    ("sonar", "perplexity"),
    ("solar-", "upstage"),
    ("phi-", "azure"),
    ("mimo-", "xiaomi"),
    ("mercury", "inception"),
];

/// Maps a canonical (bare, lowercase) model ID to its official provider ID on
/// models.dev per MP-Y8. Returns `None` for unrecognized families.
pub(crate) fn official_provider_for_model(model_id: &str) -> Option<&'static str> {
    let lower = model_id.to_ascii_lowercase();
    // OpenAI o-series: "o" followed by a digit (o1, o3-pro, o4-mini, etc.)
    if lower.starts_with('o') && lower.as_bytes().get(1).is_some_and(|b| b.is_ascii_digit()) {
        return Some("openai");
    }
    OFFICIAL_FAMILY_PROVIDERS
        .iter()
        .find(|(prefix, _)| lower.starts_with(prefix))
        .map(|(_, provider)| *provider)
}

fn positive_cost_input(variant: &SyncProviderVariant) -> Option<Decimal> {
    variant
        .cost_input
        .as_ref()
        .and_then(|raw| Decimal::from_str(raw).ok())
        .filter(|value| value.is_sign_positive() && !value.is_zero())
}

/// MP-Y7 variant selection: official provider with positive input cost,
/// otherwise the highest positive input cost.
pub(crate) fn pick_best_variant<'a>(
    model_id: &str,
    variants: &'a [SyncProviderVariant],
) -> &'a SyncProviderVariant {
    if let Some(official) = official_provider_for_model(model_id)
        && let Some(v) = variants
            .iter()
            .find(|v| v.provider_id == official && positive_cost_input(v).is_some())
    {
        return v;
    }

    variants
        .iter()
        .max_by(|a, b| {
            let cost_a = positive_cost_input(a).unwrap_or(Decimal::ZERO);
            let cost_b = positive_cost_input(b).unwrap_or(Decimal::ZERO);
            cost_a.cmp(&cost_b)
        })
        .expect("pick_best_variant called with at least one sync variant")
}

pub(crate) fn has_positive_input_variant(variants: &[SyncProviderVariant]) -> bool {
    variants.iter().any(|v| positive_cost_input(v).is_some())
}

pub(crate) fn should_ignore_sync_model(model_id: &str) -> bool {
    model_id == "auto"
        || model_id.ends_with("-thinking")
        || model_id.ends_with(":thinking")
        || model_id.ends_with("-think")
}

fn is_embedding_family(family: Option<&str>) -> bool {
    family
        .map(|s| s.to_ascii_lowercase().contains("embed"))
        .unwrap_or(false)
}

/// Reads one models.dev cost value as an exact USD-per-1M decimal string
/// (MP-Y5): the JSON token is parsed as `Decimal`, never through binary
/// floating point, and canonicalized to at most 9 fractional digits.
pub(crate) fn usd_cost_string(value: &Value) -> Option<String> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    if raw.contains(['e', 'E']) || raw.starts_with('+') {
        return None;
    }
    let decimal = Decimal::from_str_exact(&raw).ok()?;
    if decimal.is_sign_negative() {
        return None;
    }
    Some(
        decimal
            .round_dp_with_strategy(9, rust_decimal::RoundingStrategy::ToZero)
            .normalize()
            .to_string(),
    )
}

fn models_dev_variant_for_dashboard(value: &Value) -> Value {
    let mut value = value.clone();
    let Some(cost) = value
        .as_object_mut()
        .and_then(|model| model.get_mut("cost"))
        .and_then(Value::as_object_mut)
    else {
        return value;
    };
    stringify_models_dev_cost(cost);
    value
}

fn stringify_models_dev_cost(cost: &mut serde_json::Map<String, Value>) {
    for field in [
        "input",
        "output",
        "reasoning",
        "cache_read",
        "cache_write",
        "input_audio",
        "output_audio",
    ] {
        if let Some(price) = cost.get_mut(field)
            && let Value::Number(number) = price
        {
            *price = Value::String(number.to_string());
        }
    }
    if let Some(Value::Array(tiers)) = cost.get_mut("tiers") {
        for tier in tiers {
            if let Some(tier) = tier.as_object_mut() {
                stringify_models_dev_cost(tier);
            }
        }
    }
    if let Some(Value::Object(legacy)) = cost.get_mut("context_over_200k") {
        stringify_models_dev_cost(legacy);
    }
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
}

const KNOWN_PROVIDER_PREFIXES: &[&str] = &[
    "openai",
    "anthropic",
    "google",
    "xai",
    "mistral",
    "deepseek",
    "cohere",
    "meta",
    "minimax",
    "perplexity",
    "stepfun",
    "zhipuai",
    "nvidia",
    "moonshotai",
    "alibaba",
    "amazon-bedrock",
    "vercel",
    "openrouter",
    "azure",
    "groq",
    "fireworks",
    "together",
    "cloudflare",
    "replicate",
];

fn strip_provider_prefix_once<'a>(segment: &'a str, provider: &str) -> Option<&'a str> {
    segment
        .strip_prefix(&format!("{provider}--"))
        .or_else(|| segment.strip_prefix(&format!("{provider}.")))
}

fn is_known_provider_prefix(prefix: &str) -> bool {
    KNOWN_PROVIDER_PREFIXES.contains(&prefix)
}

pub fn normalize_model_id(raw: &str, provider_hint: Option<&str>) -> String {
    let mut segment = raw.rsplit('/').next().unwrap_or(raw).to_ascii_lowercase();

    if let Some(hint) = provider_hint {
        let hint = hint.to_ascii_lowercase();
        if let Some(rest) = strip_provider_prefix_once(&segment, &hint) {
            segment = rest.to_string();
        }
    }

    if let Some((prefix, _)) = segment.split_once("--")
        && is_known_provider_prefix(prefix)
        && let Some(rest) = strip_provider_prefix_once(&segment, prefix)
    {
        segment = rest.to_string();
    }

    if let Some((prefix, _)) = segment.split_once('.')
        && is_known_provider_prefix(prefix)
        && let Some(rest) = strip_provider_prefix_once(&segment, prefix)
    {
        segment = rest.to_string();
    }

    segment
}

use super::{
    AnalyticsModelBucketRow, AnalyticsProviderBucketRow, DashboardAnalyticsRaw, InsertRequestLog,
    RequestLogAffinity, RequestLogApiKey, RequestLogBilling, RequestLogChannel, RequestLogError,
    RequestLogProvider, RequestLogRow, RequestLogTiming, RequestLogTokens, RequestLogUser,
    UserStore,
};
use chrono::{Duration, Utc};
use sea_orm::Value as SeaValue;
use sea_orm::{AccessMode, ConnectionTrait, IsolationLevel, TransactionTrait};
use serde_json::Value;
use std::collections::HashMap;

const REQUEST_LOG_RETENTION_DAYS: i64 = 90;
pub(super) const REQUEST_LOG_RETENTION_INTERVAL_SECS: u64 = 3600;
const REQUEST_LOG_MODEL_FILTER_DEFAULT_MAX_TERMS: usize = 32;
const REQUEST_LOG_MODEL_FILTER_HARD_MAX_TERMS: usize = 32;
const REQUEST_LOG_MODEL_FILTER_MAX_TERMS_ENV: &str = "MONOIZE_REQUEST_LOG_MODEL_FILTER_MAX_TERMS";
const ASCII_UPPERCASE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const ASCII_LOWERCASE: &str = "abcdefghijklmnopqrstuvwxyz";

fn normalize_request_log_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}

fn request_log_model_filter_max_terms_from_raw(raw: Option<&str>) -> usize {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=REQUEST_LOG_MODEL_FILTER_HARD_MAX_TERMS).contains(value))
        .unwrap_or(REQUEST_LOG_MODEL_FILTER_DEFAULT_MAX_TERMS)
}

fn request_log_model_filter_max_terms() -> usize {
    let raw = std::env::var(REQUEST_LOG_MODEL_FILTER_MAX_TERMS_ENV).ok();
    request_log_model_filter_max_terms_from_raw(raw.as_deref())
}

fn validate_request_log_model_filter_with_limit(
    model: Option<&str>,
    max_terms: usize,
) -> Result<(), String> {
    let over_limit = model.is_some_and(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .take(max_terms.saturating_add(1))
            .count()
            > max_terms
    });
    if over_limit {
        return Err(format!(
            "request log model filter exceeds the maximum of {max_terms} terms"
        ));
    }
    Ok(())
}

fn validate_request_log_model_filter(model: Option<&str>) -> Result<(), String> {
    validate_request_log_model_filter_with_limit(model, request_log_model_filter_max_terms())
}

fn parse_optional_json_text(value: Option<String>, column: &str) -> Result<Option<Value>, String> {
    value
        .map(|raw| {
            serde_json::from_str::<Value>(&raw)
                .map_err(|error| format!("request_logs.{column}: {error}"))
        })
        .transpose()
}

fn json_nonempty_str(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

fn tried_providers_need_name_enrichment(tried: Option<&Value>) -> bool {
    let Some(Value::Array(items)) = tried else {
        return false;
    };
    items.iter().any(|item| {
        let Some(obj) = item.as_object() else {
            return false;
        };
        !json_nonempty_str(obj.get("provider_name")) || !json_nonempty_str(obj.get("channel_name"))
    })
}

pub(super) fn enrich_tried_providers_names(
    tried: &mut Option<Value>,
    provider_names: &HashMap<String, String>,
    channel_names: &HashMap<String, String>,
) {
    let Some(Value::Array(items)) = tried.as_mut() else {
        return;
    };
    for item in items {
        let Some(obj) = item.as_object_mut() else {
            continue;
        };
        if !json_nonempty_str(obj.get("provider_name")) {
            if let Some(id) = obj.get("provider_id").and_then(Value::as_str) {
                if let Some(name) = provider_names.get(id) {
                    obj.insert("provider_name".into(), Value::String(name.clone()));
                }
            }
        }
        if !json_nonempty_str(obj.get("channel_name")) {
            if let Some(id) = obj.get("channel_id").and_then(Value::as_str) {
                if let Some(name) = channel_names.get(id) {
                    obj.insert("channel_name".into(), Value::String(name.clone()));
                }
            }
        }
    }
}

fn escape_like_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn ascii_folded_like_pattern(value: &str) -> String {
    format!("%{}%", escape_like_literal(&value.to_ascii_lowercase()))
}

fn ascii_folded_sql_expression(column: &str, is_postgres: bool) -> String {
    if is_postgres {
        format!("translate({column}, '{ASCII_UPPERCASE}', '{ASCII_LOWERCASE}')")
    } else {
        format!("LOWER({column})")
    }
}

fn request_log_row_value<T: sea_orm::TryGetable>(
    row: &sea_orm::QueryResult,
    column: &str,
) -> Result<T, String> {
    row.try_get("", column)
        .map_err(|error| format!("request_logs.{column}: {error}"))
}

/// Decode a SQL boolean-ish column: PostgreSQL EXISTS yields BOOL while
/// SQLite yields INTEGER 0/1, so both decodings must be attempted.
fn row_bool(row: &sea_orm::QueryResult, column: &str) -> Result<bool, String> {
    if let Ok(value) = row.try_get::<bool>("", column) {
        return Ok(value);
    }
    match row.try_get::<i64>("", column) {
        Ok(value) => Ok(value != 0),
        Err(i64_error) => row
            .try_get::<i32>("", column)
            .map(|value| value != 0)
            .map_err(|i32_error| {
                format!(
                    "request_logs.{column}: BOOL/BIGINT decode failed ({i64_error}); INTEGER decode failed ({i32_error})"
                )
            }),
    }
}

fn row_optional_i64(row: &sea_orm::QueryResult, column: &str) -> Result<Option<i64>, String> {
    match row.try_get::<Option<i64>>("", column) {
        Ok(value) => Ok(value),
        Err(i64_error) => row
            .try_get::<Option<i32>>("", column)
            .map(|value| value.map(i64::from))
            .map_err(|i32_error| {
                format!(
                    "request_logs.{column}: BIGINT decode failed ({i64_error}); INTEGER decode failed ({i32_error})"
                )
            }),
    }
}

fn charge_aggregate_columns(is_postgres: bool) -> String {
    let digits = "(CASE WHEN SUBSTR(rl.charge_nano_usd, 1, 1) = '-' THEN SUBSTR(rl.charge_nano_usd, 2) ELSE rl.charge_nano_usd END)";
    let canonical = if is_postgres {
        "rl.charge_nano_usd ~ '^-?(0|[1-9][0-9]*)$'".to_string()
    } else {
        format!(
            "(rl.charge_nano_usd = '0' OR (SUBSTR(rl.charge_nano_usd, 1, 1) BETWEEN '1' AND '9' AND rl.charge_nano_usd NOT GLOB '*[^0-9]*') OR (SUBSTR(rl.charge_nano_usd, 1, 1) = '-' AND SUBSTR(rl.charge_nano_usd, 2, 1) BETWEEN '1' AND '9' AND {digits} NOT GLOB '*[^0-9]*'))"
        )
    };
    let in_range = format!(
        "(LENGTH({digits}) < 39 OR (LENGTH({digits}) = 39 AND ((SUBSTR(rl.charge_nano_usd, 1, 1) = '-' AND {digits} <= '170141183460469231731687303715884105728') OR (SUBSTR(rl.charge_nano_usd, 1, 1) <> '-' AND {digits} <= '170141183460469231731687303715884105727'))))"
    );

    if is_postgres {
        return format!(
            "COALESCE(SUM(CASE WHEN {canonical} AND {in_range} THEN CAST(rl.charge_nano_usd AS NUMERIC) ELSE 0 END), 0)::TEXT AS total_charge_nano_usd, COUNT(CASE WHEN {canonical} AND NOT {in_range} THEN 1 END) AS out_of_range_count"
        );
    }

    let padded = format!("('000000000000000000000000000000000000000000000' || {digits})");
    let sign = "(CASE WHEN SUBSTR(rl.charge_nano_usd, 1, 1) = '-' THEN -1 ELSE 1 END)";
    let mut select = String::new();
    for limb in 0..5 {
        if limb > 0 {
            select.push_str(", ");
        }
        let start = -9 * (limb + 1);
        select.push_str(&format!(
            "COALESCE(SUM(CASE WHEN {canonical} AND {in_range} THEN {sign} * CAST(SUBSTR({padded}, {start}, 9) AS INTEGER) ELSE 0 END), 0) AS charge_limb_{limb}"
        ));
    }
    select.push_str(&format!(
        ", COUNT(CASE WHEN {canonical} AND NOT {in_range} THEN 1 END) AS out_of_range_count"
    ));
    select
}

fn charge_aggregate_select(is_postgres: bool) -> String {
    format!("SELECT {}", charge_aggregate_columns(is_postgres))
}

/// ORDER BY expression over the charge aggregate produced by
/// `charge_aggregate_columns`, applied to the derived-table alias that owns
/// the aggregate columns. PostgreSQL orders by the numeric aggregate; SQLite
/// orders by the fixed-limb columns from most to least significant, which is
/// monotonic for the non-negative request-log charges this ranking consumes.
/// The alias indirection is required because PostgreSQL refuses to resolve
/// SELECT-list aliases inside ORDER BY expressions.
fn charge_aggregate_order_expr(is_postgres: bool, alias: &str) -> String {
    if is_postgres {
        format!("CAST({alias}.total_charge_nano_usd AS NUMERIC) DESC")
    } else {
        (0..5)
            .rev()
            .map(|limb| format!("{alias}.charge_limb_{limb} DESC"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn decode_charge_aggregate(
    row: &sea_orm::QueryResult,
    is_postgres: bool,
) -> Result<String, String> {
    let out_of_range: i64 = row
        .try_get("", "out_of_range_count")
        .map_err(|e| e.to_string())?;
    if out_of_range != 0 {
        return Err("request log charge is outside the signed i128 domain".to_string());
    }
    if is_postgres {
        let total: String = row
            .try_get("", "total_charge_nano_usd")
            .map_err(|e| e.to_string())?;
        return total
            .parse::<i128>()
            .map(|value| value.to_string())
            .map_err(|_| "request log charge aggregate overflow".to_string());
    }

    let mut total = 0i128;
    let mut scale = 1i128;
    for limb in 0..5 {
        let value: i64 = row
            .try_get("", &format!("charge_limb_{limb}"))
            .map_err(|e| e.to_string())?;
        total = total
            .checked_add(
                i128::from(value)
                    .checked_mul(scale)
                    .ok_or_else(|| "request log charge aggregate overflow".to_string())?,
            )
            .ok_or_else(|| "request log charge aggregate overflow".to_string())?;
        if limb < 4 {
            scale = scale
                .checked_mul(1_000_000_000)
                .ok_or_else(|| "request log charge aggregate overflow".to_string())?;
        }
    }
    Ok(total.to_string())
}

fn analytics_bucket_expr(is_sqlite: bool) -> &'static str {
    if is_sqlite {
        "CAST(((rl.created_at_unix_ms - $1) * $2) / $3 AS BIGINT)"
    } else {
        "FLOOR(((rl.created_at_unix_ms - $1)::NUMERIC * $2) / $3)::BIGINT"
    }
}

fn analytics_model_bucket_sql(is_sqlite: bool, user_scoped: bool) -> String {
    let bucket_expr = analytics_bucket_expr(is_sqlite);
    let charge_columns = charge_aggregate_columns(!is_sqlite);
    let user_filter = if user_scoped {
        " AND rl.user_id = $6"
    } else {
        ""
    };
    format!(
        "SELECT {bucket_expr} AS bucket_idx, rl.model, {charge_columns}, COUNT(*) AS call_count \
         FROM request_logs rl \
         WHERE rl.created_at_unix_ms >= $4 AND rl.created_at_unix_ms < $5{user_filter} \
         GROUP BY bucket_idx, rl.model \
         ORDER BY bucket_idx, rl.model"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        analytics_bucket_expr, analytics_model_bucket_sql, append_request_log_filters,
        ascii_folded_like_pattern, charge_aggregate_select, decode_charge_aggregate,
        enrich_tried_providers_names, escape_like_literal,
        request_log_model_filter_max_terms_from_raw, tried_providers_need_name_enrichment,
        validate_request_log_model_filter_with_limit,
    };
    use crate::db::DbPool;
    use sea_orm::{ConnectionTrait, TransactionTrait, Value as SeaValue};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn like_literals_escape_wildcards_and_escape_character() {
        assert_eq!(escape_like_literal(r"A%_\\B"), r"A\%\_\\\\B");
        assert_eq!(ascii_folded_like_pattern("CAFÉ"), "%cafÉ%");
    }

    #[test]
    fn tried_providers_name_enrichment_fills_missing_names_and_preserves_present_names() {
        let mut tried = Some(json!([
            {
                "attempt_number": 1,
                "provider_id": "prov-1",
                "channel_id": "ch-1",
                "error": "upstream status 429"
            },
            {
                "attempt_number": 2,
                "provider_id": "prov-2",
                "channel_id": "ch-2",
                "provider_name": "Kept Provider",
                "channel_name": "Kept Channel",
                "error": "upstream status 502"
            }
        ]));
        assert!(tried_providers_need_name_enrichment(tried.as_ref()));
        let provider_names = HashMap::from([
            ("prov-1".to_string(), "Ciii".to_string()),
            ("prov-2".to_string(), "Should Not Replace".to_string()),
        ]);
        let channel_names = HashMap::from([
            ("ch-1".to_string(), "ciii_1".to_string()),
            ("ch-2".to_string(), "Should Not Replace".to_string()),
        ]);
        enrich_tried_providers_names(&mut tried, &provider_names, &channel_names);
        assert_eq!(
            tried,
            Some(json!([
                {
                    "attempt_number": 1,
                    "provider_id": "prov-1",
                    "channel_id": "ch-1",
                    "provider_name": "Ciii",
                    "channel_name": "ciii_1",
                    "error": "upstream status 429"
                },
                {
                    "attempt_number": 2,
                    "provider_id": "prov-2",
                    "channel_id": "ch-2",
                    "provider_name": "Kept Provider",
                    "channel_name": "Kept Channel",
                    "error": "upstream status 502"
                }
            ]))
        );
        assert!(!tried_providers_need_name_enrichment(tried.as_ref()));
    }

    #[test]
    fn postgres_filters_use_ascii_translate_and_prefold_bind_values() {
        let mut sql = "SELECT 1 FROM request_logs rl WHERE 1 = 1".to_string();
        let mut values = Vec::new();
        let mut idx = 1;
        append_request_log_filters(
            &mut sql,
            &mut values,
            &mut idx,
            true,
            Some("CAFé"),
            None,
            None,
            None,
            Some("cafÉ"),
            None,
            None,
        )
        .unwrap();

        assert!(sql.contains("translate(rl.model"));
        assert!(sql.contains("translate(rl.upstream_model"));
        assert!(!sql.contains("LOWER("));
        assert_eq!(idx, 6);
        let SeaValue::String(Some(model_pattern)) = &values[0] else {
            panic!("model filter must bind text");
        };
        assert_eq!(model_pattern.as_str(), "%café%");
        for value in &values[1..] {
            let SeaValue::String(Some(search_pattern)) = value else {
                panic!("search filter must bind text");
            };
            assert_eq!(search_pattern.as_str(), "%cafÉ%");
        }
    }

    #[test]
    fn model_filter_limit_configuration_is_positive_and_hard_capped() {
        assert_eq!(request_log_model_filter_max_terms_from_raw(None), 32);
        assert_eq!(request_log_model_filter_max_terms_from_raw(Some("")), 32);
        assert_eq!(request_log_model_filter_max_terms_from_raw(Some(" 7 ")), 7);
        assert_eq!(request_log_model_filter_max_terms_from_raw(Some("0")), 32);
        assert_eq!(request_log_model_filter_max_terms_from_raw(Some("-1")), 32);
        assert_eq!(request_log_model_filter_max_terms_from_raw(Some("33")), 32);
        assert_eq!(request_log_model_filter_max_terms_from_raw(Some("bad")), 32);
    }

    #[test]
    fn model_filter_limit_counts_nonempty_and_duplicate_terms() {
        assert!(
            validate_request_log_model_filter_with_limit(Some("a, ,b"), 2).is_ok(),
            "empty terms are discarded"
        );
        assert!(
            validate_request_log_model_filter_with_limit(Some("a,a,b"), 2).is_err(),
            "duplicate terms still create predicates"
        );

        let mut sql = "SELECT 1 FROM request_logs rl WHERE 1 = 1".to_string();
        let mut values = Vec::new();
        let mut idx = 1;
        let over_limit = (0..33)
            .map(|term| format!("model-{term}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            append_request_log_filters(
                &mut sql,
                &mut values,
                &mut idx,
                false,
                Some(&over_limit),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .is_err()
        );
        assert_eq!(sql, "SELECT 1 FROM request_logs rl WHERE 1 = 1");
        assert!(values.is_empty());
        assert_eq!(idx, 1);
    }

    #[tokio::test]
    async fn sqlite_charge_aggregate_is_exact_and_ignores_noncanonical_text() {
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        db.write()
            .await
            .execute_unprepared("CREATE TABLE request_logs (charge_nano_usd TEXT)")
            .await
            .unwrap();
        for value in [
            "9223372036854775807",
            "1",
            "170141183460469231731687303715884105727",
            "-170141183460469231731687303715884105728",
            "+9",
            "01",
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (charge_nano_usd) VALUES ($1)",
                    vec![value.into()],
                ))
                .await
                .unwrap();
        }

        let row = db
            .read()
            .query_one(db.stmt(
                &format!("{} FROM request_logs rl", charge_aggregate_select(false)),
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decode_charge_aggregate(&row, false).unwrap(),
            "9223372036854775807"
        );
    }

    #[tokio::test]
    async fn sqlite_charge_aggregate_rejects_canonical_value_outside_i128() {
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        db.write()
            .await
            .execute_unprepared("CREATE TABLE request_logs (charge_nano_usd TEXT)")
            .await
            .unwrap();
        db.write()
            .await
            .execute(db.stmt(
                "INSERT INTO request_logs (charge_nano_usd) VALUES ($1)",
                vec!["170141183460469231731687303715884105728".into()],
            ))
            .await
            .unwrap();

        let row = db
            .read()
            .query_one(db.stmt(
                &format!("{} FROM request_logs rl", charge_aggregate_select(false)),
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decode_charge_aggregate(&row, false).unwrap_err(),
            "request log charge is outside the signed i128 domain"
        );
    }

    #[tokio::test]
    async fn sqlite_analytics_model_buckets_group_and_decode_exact_charges() {
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        db.write()
            .await
            .execute_unprepared(
                "CREATE TABLE request_logs (created_at_unix_ms INTEGER NOT NULL, model TEXT NOT NULL, charge_nano_usd TEXT, user_id TEXT)",
            )
            .await
            .unwrap();
        for (created_at_unix_ms, model, charge, user_id) in [
            (100_i64, "exact", "9223372036854775807", "u1"),
            (200_i64, "exact", "1", "u1"),
            (300_i64, "exact", "+9", "u1"),
            (
                400_i64,
                "out-of-range",
                "170141183460469231731687303715884105728",
                "u1",
            ),
            (
                600_i64,
                "overflow",
                "170141183460469231731687303715884105727",
                "u1",
            ),
            (700_i64, "overflow", "1", "u1"),
            (100_i64, "excluded", "99", "u2"),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (created_at_unix_ms, model, charge_nano_usd, user_id) VALUES ($1, $2, $3, $4)",
                    vec![
                        created_at_unix_ms.into(),
                        model.into(),
                        charge.into(),
                        user_id.into(),
                    ],
                ))
                .await
                .unwrap();
        }

        let sql = analytics_model_bucket_sql(true, true);
        assert!(sql.contains("GROUP BY bucket_idx, rl.model"));
        assert!(!sql.contains("SELECT rl.created_at_unix_ms"));
        let rows = db
            .read()
            .query_all(db.stmt(
                &sql,
                vec![
                    0_i64.into(),
                    2_i64.into(),
                    1_000_i64.into(),
                    0_i64.into(),
                    1_000_i64.into(),
                    "u1".into(),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);

        let mut groups = std::collections::BTreeMap::new();
        for row in rows {
            let model: String = row.try_get("", "model").unwrap();
            let bucket_idx: i64 = row.try_get("", "bucket_idx").unwrap();
            let call_count: i64 = row.try_get("", "call_count").unwrap();
            groups.insert(
                model,
                (bucket_idx, call_count, decode_charge_aggregate(&row, false)),
            );
        }

        assert_eq!(groups["exact"].0, 0);
        assert_eq!(groups["exact"].1, 3);
        assert_eq!(groups["exact"].2.as_deref().unwrap(), "9223372036854775808");
        assert_eq!(
            groups["out-of-range"].2.as_ref().unwrap_err(),
            "request log charge is outside the signed i128 domain"
        );
        assert_eq!(groups["overflow"].0, 1);
        assert_eq!(
            groups["overflow"].2.as_ref().unwrap_err(),
            "request log charge aggregate overflow"
        );
        assert!(!groups.contains_key("excluded"));
    }

    #[tokio::test]
    async fn sqlite_filters_fold_ascii_only_and_keep_non_ascii_case_distinct() {
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        db.write()
            .await
            .execute_unprepared(
                "CREATE TABLE request_logs (model TEXT, upstream_model TEXT, request_id TEXT, request_ip TEXT, status TEXT, api_key_id TEXT, request_kind TEXT, user_id TEXT, created_at_unix_ms INTEGER, created_at TEXT)",
            )
            .await
            .unwrap();
        for model in ["CAFÉ", "café", "cafe"] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (model, upstream_model, request_id, request_ip, created_at) VALUES ($1, '', '', '', '')",
                    vec![model.into()],
                ))
                .await
                .unwrap();
        }

        let mut model_sql = "SELECT model FROM request_logs rl WHERE 1 = 1".to_string();
        let mut model_values = Vec::new();
        let mut model_idx = 1;
        append_request_log_filters(
            &mut model_sql,
            &mut model_values,
            &mut model_idx,
            false,
            Some("CAFé"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let rows = db
            .read()
            .query_all(db.stmt(&model_sql, model_values))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<String>("", "model").unwrap(), "café");

        let mut search_sql = "SELECT model FROM request_logs rl WHERE 1 = 1".to_string();
        let mut search_values = Vec::new();
        let mut search_idx = 1;
        append_request_log_filters(
            &mut search_sql,
            &mut search_values,
            &mut search_idx,
            false,
            None,
            None,
            None,
            None,
            Some("cafÉ"),
            None,
            None,
        )
        .unwrap();
        let rows = db
            .read()
            .query_all(db.stmt(&search_sql, search_values))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<String>("", "model").unwrap(), "CAFÉ");
    }

    #[tokio::test]
    async fn sqlite_filters_are_literal_case_insensitive_and_include_legacy_time_rows() {
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        db.write()
            .await
            .execute_unprepared(
                "CREATE TABLE request_logs (model TEXT, upstream_model TEXT, request_id TEXT, request_ip TEXT, status TEXT, api_key_id TEXT, request_kind TEXT, user_id TEXT, created_at_unix_ms INTEGER, created_at TEXT)",
            )
            .await
            .unwrap();
        for (model, unix_ms, created_at) in [
            (
                "GPT%_Model",
                Some(1_704_067_200_000_i64),
                "2024-01-01T00:00:00+00:00",
            ),
            (
                "gptXXmodel",
                Some(1_704_067_200_000_i64),
                "2024-01-01T00:00:00+00:00",
            ),
            ("gPt%_mOdEl-legacy", None, "2024-01-01T00:30:00+00:00"),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (model, upstream_model, request_id, request_ip, created_at_unix_ms, created_at) VALUES ($1, '', '', '', $2, $3)",
                    vec![model.into(), SeaValue::BigInt(unix_ms), created_at.into()],
                ))
                .await
                .unwrap();
        }

        let mut sql = "SELECT model FROM request_logs rl WHERE 1 = 1".to_string();
        let mut values = Vec::new();
        let mut idx = 1;
        append_request_log_filters(
            &mut sql,
            &mut values,
            &mut idx,
            false,
            Some("gpt%_model"),
            None,
            None,
            None,
            None,
            Some("2024-01-01T00:00:00Z"),
            Some("2024-01-01T01:00:00Z"),
        )
        .unwrap();
        assert_eq!(idx, values.len() + 1);
        let rows = db.read().query_all(db.stmt(&sql, values)).await.unwrap();
        let models = rows
            .iter()
            .map(|row| row.try_get::<String>("", "model").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(models, vec!["GPT%_Model", "gPt%_mOdEl-legacy"]);

        let bucket = db
            .read()
            .query_one(db.stmt(
                &format!(
                    "SELECT {} AS bucket_idx FROM request_logs rl WHERE model = $4",
                    analytics_bucket_expr(true)
                ),
                vec![
                    1_704_067_199_400_i64.into(),
                    1_i64.into(),
                    1_000_i64.into(),
                    "GPT%_Model".into(),
                ],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bucket.try_get::<i64>("", "bucket_idx").unwrap(), 0);

        let mut malformed_sql = "SELECT 1 FROM request_logs rl WHERE 1 = 1".to_string();
        let mut malformed_values = Vec::new();
        let mut malformed_idx = 1;
        assert!(
            append_request_log_filters(
                &mut malformed_sql,
                &mut malformed_values,
                &mut malformed_idx,
                false,
                None,
                None,
                None,
                None,
                None,
                Some("bad"),
                None,
            )
            .is_err()
        );
        assert_eq!(malformed_idx, 1);
        assert!(malformed_values.is_empty());
    }

    #[tokio::test]
    async fn postgres_request_log_semantics_match_sqlite_when_test_dsn_is_configured() {
        let Some(dsn) = std::env::var("MONOIZE_TEST_POSTGRES_DSN")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return;
        };
        let db = DbPool::connect(&dsn).await.unwrap();
        let txn = db.read().begin().await.unwrap();
        txn.execute_unprepared(
            "CREATE TEMP TABLE request_logs (model TEXT, upstream_model TEXT, request_id TEXT, request_ip TEXT, status TEXT, api_key_id TEXT, request_kind TEXT, user_id TEXT, created_at_unix_ms BIGINT, created_at TEXT, charge_nano_usd TEXT)",
        )
        .await
        .unwrap();
        for (model, charge) in [("GPT%_Model", "9223372036854775808"), ("gptXXmodel", "4")] {
            txn.execute(db.stmt(
                "INSERT INTO request_logs (model, upstream_model, request_id, request_ip, created_at_unix_ms, created_at, charge_nano_usd) VALUES ($1, '', '', '', $2, $3, $4)",
                vec![
                    model.into(),
                    1_704_067_200_000_i64.into(),
                    "2024-01-01T00:00:00+00:00".into(),
                    charge.into(),
                ],
            ))
            .await
            .unwrap();
        }
        for model in ["CAFÉ", "café", "cafe"] {
            txn.execute(db.stmt(
                "INSERT INTO request_logs (model, upstream_model, request_id, request_ip, created_at_unix_ms, created_at) VALUES ($1, '', '', '', $2, $3)",
                vec![
                    model.into(),
                    1_704_067_200_000_i64.into(),
                    "2024-01-01T00:00:00+00:00".into(),
                ],
            ))
            .await
            .unwrap();
        }

        let mut sql = "SELECT model FROM request_logs rl WHERE 1 = 1".to_string();
        let mut values = Vec::new();
        let mut idx = 1;
        append_request_log_filters(
            &mut sql,
            &mut values,
            &mut idx,
            true,
            Some("gpt%_model"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let rows = txn.query_all(db.stmt(&sql, values)).await.unwrap();
        assert_eq!(rows.len(), 1);

        let mut non_ascii_sql = "SELECT model FROM request_logs rl WHERE 1 = 1".to_string();
        let mut non_ascii_values = Vec::new();
        let mut non_ascii_idx = 1;
        append_request_log_filters(
            &mut non_ascii_sql,
            &mut non_ascii_values,
            &mut non_ascii_idx,
            true,
            Some("CAFé"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(non_ascii_sql.contains("translate(rl.model"));
        let rows = txn
            .query_all(db.stmt(&non_ascii_sql, non_ascii_values))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<String>("", "model").unwrap(), "café");

        let mut non_ascii_search_sql = "SELECT model FROM request_logs rl WHERE 1 = 1".to_string();
        let mut non_ascii_search_values = Vec::new();
        let mut non_ascii_search_idx = 1;
        append_request_log_filters(
            &mut non_ascii_search_sql,
            &mut non_ascii_search_values,
            &mut non_ascii_search_idx,
            true,
            None,
            None,
            None,
            None,
            Some("cafÉ"),
            None,
            None,
        )
        .unwrap();
        let rows = txn
            .query_all(db.stmt(&non_ascii_search_sql, non_ascii_search_values))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].try_get::<String>("", "model").unwrap(), "CAFÉ");

        let aggregate = txn
            .query_one(db.stmt(
                &format!("{} FROM request_logs rl", charge_aggregate_select(true)),
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decode_charge_aggregate(&aggregate, true).unwrap(),
            "9223372036854775812"
        );

        let bucket_sql = format!(
            "SELECT {} AS bucket_idx FROM request_logs rl WHERE model = $4",
            analytics_bucket_expr(false)
        );
        let bucket = txn
            .query_one(db.stmt(
                &bucket_sql,
                vec![
                    1_704_067_199_400_i64.into(),
                    1_i64.into(),
                    1_000_i64.into(),
                    "GPT%_Model".into(),
                ],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bucket.try_get::<i64>("", "bucket_idx").unwrap(), 0);

        for (model, charge) in [
            ("analytics-exact", "9223372036854775807"),
            ("analytics-exact", "1"),
            (
                "analytics-out-of-range",
                "170141183460469231731687303715884105728",
            ),
            (
                "analytics-overflow",
                "170141183460469231731687303715884105727",
            ),
            ("analytics-overflow", "1"),
        ] {
            txn.execute(db.stmt(
                "INSERT INTO request_logs (model, upstream_model, request_id, request_ip, user_id, created_at_unix_ms, created_at, charge_nano_usd) VALUES ($1, '', '', '', 'u1', $2, $3, $4)",
                vec![
                    model.into(),
                    1_704_067_200_000_i64.into(),
                    "2024-01-01T00:00:00+00:00".into(),
                    charge.into(),
                ],
            ))
            .await
            .unwrap();
        }
        let analytics_rows = txn
            .query_all(db.stmt(
                &analytics_model_bucket_sql(false, true),
                vec![
                    1_704_067_199_000_i64.into(),
                    2_i64.into(),
                    2_000_i64.into(),
                    1_704_067_199_000_i64.into(),
                    1_704_067_201_000_i64.into(),
                    "u1".into(),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(analytics_rows.len(), 3);
        let mut analytics_groups = std::collections::BTreeMap::new();
        for row in analytics_rows {
            let model: String = row.try_get("", "model").unwrap();
            analytics_groups.insert(model, decode_charge_aggregate(&row, true));
        }
        assert_eq!(
            analytics_groups["analytics-exact"].as_deref().unwrap(),
            "9223372036854775808"
        );
        assert_eq!(
            analytics_groups["analytics-out-of-range"]
                .as_ref()
                .unwrap_err(),
            "request log charge is outside the signed i128 domain"
        );
        assert_eq!(
            analytics_groups["analytics-overflow"].as_ref().unwrap_err(),
            "request log charge aggregate overflow"
        );
        txn.rollback().await.unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn append_request_log_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    idx: &mut usize,
    is_postgres: bool,
    model: Option<&str>,
    status: Option<&str>,
    api_key_id: Option<&str>,
    username: Option<&str>,
    search: Option<&str>,
    time_from: Option<&str>,
    time_to: Option<&str>,
) -> Result<(), String> {
    if let Some(model) = model {
        validate_request_log_model_filter(Some(model))?;
        let folded_model = ascii_folded_sql_expression("rl.model", is_postgres);
        let models: Vec<&str> = model
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if models.len() == 1 {
            sql.push_str(&format!(" AND {folded_model} LIKE ${} ESCAPE '\\'", *idx,));
            values.push(ascii_folded_like_pattern(models[0]).into());
            *idx += 1;
        } else if !models.is_empty() {
            sql.push_str(" AND (");
            for (i, m) in models.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str(&format!("{folded_model} LIKE ${} ESCAPE '\\'", *idx,));
                values.push(ascii_folded_like_pattern(m).into());
                *idx += 1;
            }
            sql.push(')');
        }
    }
    if let Some(status) = status {
        sql.push_str(&format!(" AND rl.status = ${}", *idx));
        values.push(status.into());
        *idx += 1;
    }
    if let Some(api_key_id) = api_key_id {
        sql.push_str(&format!(" AND rl.api_key_id = ${}", *idx));
        values.push(api_key_id.into());
        *idx += 1;
    }
    if let Some(username) = username {
        sql.push_str(&format!(" AND (rl.user_id IN (SELECT id FROM users WHERE username = ${}) OR rl.request_kind = 'active_probe_connectivity')", *idx));
        values.push(username.into());
        *idx += 1;
    }
    if let Some(search) = search {
        let search_like = ascii_folded_like_pattern(search);
        let model = ascii_folded_sql_expression("rl.model", is_postgres);
        let upstream_model = ascii_folded_sql_expression("rl.upstream_model", is_postgres);
        let request_id = ascii_folded_sql_expression("rl.request_id", is_postgres);
        let request_ip = ascii_folded_sql_expression("rl.request_ip", is_postgres);
        sql.push_str(&format!(
            " AND ({model} LIKE ${i} ESCAPE '\\' OR {upstream_model} LIKE ${j} ESCAPE '\\' OR {request_id} LIKE ${k} ESCAPE '\\' OR {request_ip} LIKE ${l} ESCAPE '\\')",
            i = *idx, j = *idx + 1, k = *idx + 2, l = *idx + 3
        ));
        values.push(search_like.clone().into());
        values.push(search_like.clone().into());
        values.push(search_like.clone().into());
        values.push(search_like.into());
        *idx += 4;
    }
    if let Some(time_from) = time_from {
        let parsed = chrono::DateTime::parse_from_rfc3339(time_from)
            .map_err(|_| "invalid time_from RFC 3339 timestamp".to_string())?
            .with_timezone(&Utc);
        sql.push_str(&format!(
            " AND ((rl.created_at_unix_ms IS NOT NULL AND rl.created_at_unix_ms >= ${}) OR (rl.created_at_unix_ms IS NULL AND rl.created_at >= ${}))",
            *idx,
            *idx + 1
        ));
        values.push(parsed.timestamp_millis().into());
        values.push(parsed.to_rfc3339().into());
        *idx += 2;
    }
    if let Some(time_to) = time_to {
        let parsed = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|_| "invalid time_to RFC 3339 timestamp".to_string())?
            .with_timezone(&Utc);
        sql.push_str(&format!(
            " AND ((rl.created_at_unix_ms IS NOT NULL AND rl.created_at_unix_ms < ${}) OR (rl.created_at_unix_ms IS NULL AND rl.created_at < ${}))",
            *idx,
            *idx + 1
        ));
        values.push(parsed.timestamp_millis().into());
        values.push(parsed.to_rfc3339().into());
        *idx += 2;
    }
    Ok(())
}

fn row_to_request_log(row: &sea_orm::QueryResult) -> Result<RequestLogRow, String> {
    let is_stream = request_log_row_value::<i32>(row, "is_stream")? == 1;
    let charge_nano_usd = request_log_row_value(row, "charge_nano_usd")?;
    let provider_multiplier = request_log_row_value::<Option<String>>(row, "provider_multiplier")?
        .map(|value| {
            value
                .parse()
                .map_err(|error| format!("request_logs.provider_multiplier: {error}"))
        })
        .transpose()?;

    Ok(RequestLogRow {
        id: request_log_row_value(row, "id")?,
        request_id: request_log_row_value(row, "request_id")?,
        created_at: request_log_row_value(row, "created_at")?,
        status: request_log_row_value(row, "status")?,
        is_stream,
        model: request_log_row_value(row, "model")?,
        upstream_model: request_log_row_value(row, "upstream_model")?,
        effective_provider_type: request_log_row_value(row, "effective_provider_type")?,
        request_kind: request_log_row_value(row, "request_kind")?,
        reasoning_effort: request_log_row_value(row, "reasoning_effort")?,
        request_ip: request_log_row_value(row, "request_ip")?,
        tried_providers: parse_optional_json_text(
            request_log_row_value(row, "tried_providers_json")?,
            "tried_providers_json",
        )?,
        session_affinity_value: request_log_row_value(row, "session_affinity_value")?,
        has_capture: row_bool(row, "has_capture")?,
        provider: RequestLogProvider {
            id: request_log_row_value(row, "provider_id")?,
            name: request_log_row_value(row, "provider_name")?,
            multiplier: provider_multiplier,
        },
        channel: RequestLogChannel {
            id: request_log_row_value(row, "channel_id")?,
            name: request_log_row_value(row, "channel_name")?,
        },
        affinity: RequestLogAffinity {
            hit: request_log_row_value::<Option<i32>>(row, "affinity_hit")?.map(|v| v != 0),
            key_hash: request_log_row_value(row, "affinity_key_hash")?,
            target: request_log_row_value(row, "affinity_target")?,
        },
        user: RequestLogUser {
            id: request_log_row_value(row, "user_id")?,
            username: request_log_row_value(row, "username")?,
        },
        api_key: RequestLogApiKey {
            id: request_log_row_value(row, "api_key_id")?,
            name: request_log_row_value(row, "api_key_name")?,
        },
        tokens: RequestLogTokens {
            input: row_optional_i64(row, "input_tokens")?,
            output: row_optional_i64(row, "output_tokens")?,
            cache_read: row_optional_i64(row, "cache_read_tokens")?,
            cache_creation: row_optional_i64(row, "cache_creation_tokens")?,
            tool_prompt: row_optional_i64(row, "tool_prompt_tokens")?,
            reasoning: row_optional_i64(row, "reasoning_tokens")?,
            accepted_prediction: row_optional_i64(row, "accepted_prediction_tokens")?,
            rejected_prediction: row_optional_i64(row, "rejected_prediction_tokens")?,
        },
        timing: {
            let duration_ms = row_optional_i64(row, "duration_ms")?;
            let ttfb_ms = row_optional_i64(row, "ttfb_ms")?;
            RequestLogTiming {
                duration_ms,
                ttfb_ms,
                duration_ms_alias: duration_ms,
                elapsed_ms: duration_ms,
                latency_ms: duration_ms,
                ttfb_ms_alias: ttfb_ms,
                first_token_ms: ttfb_ms,
                first_token_ms_alias: ttfb_ms,
            }
        },
        billing: RequestLogBilling {
            charge_nano_usd,
            breakdown: parse_optional_json_text(
                request_log_row_value(row, "billing_breakdown_json")?,
                "billing_breakdown_json",
            )?,
        },
        usage: parse_optional_json_text(
            request_log_row_value(row, "usage_breakdown_json")?,
            "usage_breakdown_json",
        )?,
        error: RequestLogError {
            code: request_log_row_value(row, "error_code")?,
            message: request_log_row_value(row, "error_message")?,
            http_status: row_optional_i64(row, "error_http_status")?,
        },
    })
}

impl UserStore {
    pub(crate) fn validate_request_log_model_filter(model: Option<&str>) -> Result<(), String> {
        validate_request_log_model_filter(model)
    }

    pub fn reserve_terminal_request_log(
        &self,
    ) -> Result<crate::db_cache::RequestLogReservation, String> {
        self.request_log_batcher
            .reserve_terminal_log()
            .map_err(|error| error.to_string())
    }

    pub async fn arm_terminal_request_log(
        &self,
        fallback_log: InsertRequestLog,
        reservation: &crate::db_cache::RequestLogReservation,
    ) -> Result<(), String> {
        self.request_log_batcher
            .arm_reserved(fallback_log, reservation)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn cancel_terminal_request_log(
        &self,
        reservation: &crate::db_cache::RequestLogReservation,
    ) -> Result<(), String> {
        self.request_log_batcher
            .cancel_reserved(reservation)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn cleanup_expired_request_logs(&self) -> Result<u64, String> {
        let cutoff_unix_ms =
            (Utc::now() - Duration::days(REQUEST_LOG_RETENTION_DAYS)).timestamp_millis();
        let result = self.db.write().await
            .execute(self.db.stmt(
                "DELETE FROM request_logs WHERE created_at_unix_ms IS NOT NULL AND created_at_unix_ms < $1",
                vec![cutoff_unix_ms.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    pub async fn cleanup_pending_request_logs(&self) -> Result<u64, String> {
        let result = self.db.write().await
            .execute(self.db.stmt(
                "UPDATE request_logs SET status = 'error', error_code = 'server_shutdown', error_message = 'interrupted by server restart' WHERE status = 'pending'",
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    pub async fn insert_request_log_pending(
        &self,
        _request_id: &str,
        _user_id: &str,
        _api_key_id: Option<&str>,
        _model: &str,
        _is_stream: bool,
        _request_ip: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    pub async fn update_pending_request_log_channel(
        &self,
        _user_id: &str,
        _request_id: &str,
        _provider_id: &str,
        _channel_id: &str,
        _upstream_model: &str,
        _provider_multiplier: crate::exact_decimal::Multiplier,
    ) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_pending_request_log_usage(
        &self,
        _user_id: &str,
        _request_id: &str,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_read_tokens: Option<u64>,
        _cache_creation_tokens: Option<u64>,
        _tool_prompt_tokens: Option<u64>,
        _reasoning_tokens: Option<u64>,
        _accepted_prediction_tokens: Option<u64>,
        _rejected_prediction_tokens: Option<u64>,
        _usage_breakdown_json: Option<Value>,
    ) -> Result<(), String> {
        Ok(())
    }

    pub async fn finalize_request_log(&self, log: InsertRequestLog) -> Result<(), String> {
        self.request_log_batcher
            .push(log)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn finalize_reserved_request_log(
        &self,
        log: InsertRequestLog,
        reservation: crate::db_cache::RequestLogReservation,
    ) -> Result<(), String> {
        self.request_log_batcher
            .push_reserved(log, reservation)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn insert_request_log(&self, log: InsertRequestLog) -> Result<(), String> {
        self.request_log_batcher
            .push(log)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn load_routing_name_maps(
        &self,
    ) -> Result<(HashMap<String, String>, HashMap<String, String>), String> {
        let provider_rows = self
            .db
            .read()
            .query_all(
                self.db
                    .stmt("SELECT id, name FROM monoize_providers", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut provider_names = HashMap::new();
        for row in provider_rows {
            let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let name: String = row.try_get("", "name").map_err(|e| e.to_string())?;
            if !name.trim().is_empty() {
                provider_names.insert(id, name);
            }
        }
        let channel_rows = self
            .db
            .read()
            .query_all(
                self.db
                    .stmt("SELECT id, name FROM monoize_channels", vec![]),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut channel_names = HashMap::new();
        for row in channel_rows {
            let id: String = row.try_get("", "id").map_err(|e| e.to_string())?;
            let name: String = row.try_get("", "name").map_err(|e| e.to_string())?;
            if !name.trim().is_empty() {
                channel_names.insert(id, name);
            }
        }
        Ok((provider_names, channel_names))
    }

    async fn enrich_request_log_tried_provider_names(
        &self,
        logs: &mut [RequestLogRow],
    ) -> Result<(), String> {
        let needs_enrichment = logs
            .iter()
            .any(|log| tried_providers_need_name_enrichment(log.tried_providers.as_ref()));
        if !needs_enrichment {
            return Ok(());
        }
        let (provider_names, channel_names) = self.load_routing_name_maps().await?;
        for log in logs {
            enrich_tried_providers_names(&mut log.tried_providers, &provider_names, &channel_names);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_request_logs_by_user(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
        model: Option<&str>,
        status: Option<&str>,
        api_key_id: Option<&str>,
        search: Option<&str>,
        time_from: Option<&str>,
        time_to: Option<&str>,
    ) -> Result<(Vec<RequestLogRow>, i64, String), String> {
        Self::validate_request_log_model_filter(model)?;
        let is_postgres = self.db.is_postgres();
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let search = normalize_request_log_filter(search);
        let txn = self
            .db
            .read()
            .begin_with_config(
                is_postgres.then_some(IsolationLevel::RepeatableRead),
                is_postgres.then_some(AccessMode::ReadOnly),
            )
            .await
            .map_err(|e| e.to_string())?;

        // Count query
        let mut count_sql =
            "SELECT COUNT(*) as cnt FROM request_logs rl WHERE rl.user_id = $1".to_string();
        let mut count_values: Vec<SeaValue> = vec![user_id.into()];
        let mut count_idx = 2usize;
        append_request_log_filters(
            &mut count_sql,
            &mut count_values,
            &mut count_idx,
            is_postgres,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
            time_from,
            time_to,
        )?;
        let count_row = txn
            .query_one(self.db.stmt(&count_sql, count_values))
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = count_row
            .ok_or_else(|| "no count row".to_string())?
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;

        // Sum query
        let mut sum_sql = format!(
            "{} FROM request_logs rl WHERE rl.user_id = $1",
            charge_aggregate_select(is_postgres)
        );
        let mut sum_values: Vec<SeaValue> = vec![user_id.into()];
        let mut sum_idx = 2usize;
        append_request_log_filters(
            &mut sum_sql,
            &mut sum_values,
            &mut sum_idx,
            is_postgres,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
            time_from,
            time_to,
        )?;
        let sum_row = txn
            .query_one(self.db.stmt(&sum_sql, sum_values))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no request log charge aggregate row".to_string())?;
        let total_charge_nano_usd = decode_charge_aggregate(&sum_row, is_postgres)?;

        // Rows query
        let mut rows_sql = r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.input_tokens, rl.output_tokens, rl.cache_read_tokens, rl.cache_creation_tokens,
                      rl.tool_prompt_tokens, rl.reasoning_tokens,
                      rl.accepted_prediction_tokens, rl.rejected_prediction_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms,
                      rl.request_ip, rl.reasoning_effort, rl.tried_providers_json, rl.request_kind,
                      rl.effective_provider_type, rl.affinity_hit, rl.affinity_key_hash, rl.affinity_target,
                      rl.session_affinity_value,
                      rl.created_at,
                      EXISTS (SELECT 1 FROM request_capture_records rcr WHERE rcr.request_id = rl.request_id AND rcr.user_id = rl.user_id) AS has_capture,
                      u.username AS username, ak.name AS api_key_name, ch.name AS channel_name, p.name AS provider_name
               FROM request_logs rl
               LEFT JOIN users u ON u.id = rl.user_id
               LEFT JOIN api_keys ak ON ak.id = rl.api_key_id
               LEFT JOIN monoize_channels ch ON ch.id = rl.channel_id
               LEFT JOIN monoize_providers p ON p.id = rl.provider_id
               WHERE rl.user_id = $1"#
            .to_string();
        let mut rows_values: Vec<SeaValue> = vec![user_id.into()];
        let mut rows_idx = 2usize;
        append_request_log_filters(
            &mut rows_sql,
            &mut rows_values,
            &mut rows_idx,
            is_postgres,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            None,
            search.as_deref(),
            time_from,
            time_to,
        )?;
        if is_postgres {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC NULLS LAST, rl.created_at DESC, rl.id DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        } else {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC, rl.created_at DESC, rl.id DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        }
        rows_values.push(SeaValue::BigInt(Some(limit)));
        rows_values.push(SeaValue::BigInt(Some(offset)));

        let rows = txn
            .query_all(self.db.stmt(&rows_sql, rows_values))
            .await
            .map_err(|e| e.to_string())?;

        txn.commit().await.map_err(|e| e.to_string())?;
        let mut logs = rows
            .into_iter()
            .map(|row| row_to_request_log(&row))
            .collect::<Result<Vec<_>, _>>()?;
        self.enrich_request_log_tried_provider_names(&mut logs)
            .await?;

        Ok((logs, total, total_charge_nano_usd))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_all_request_logs(
        &self,
        limit: i64,
        offset: i64,
        model: Option<&str>,
        status: Option<&str>,
        api_key_id: Option<&str>,
        username: Option<&str>,
        search: Option<&str>,
        time_from: Option<&str>,
        time_to: Option<&str>,
    ) -> Result<(Vec<RequestLogRow>, i64, String), String> {
        Self::validate_request_log_model_filter(model)?;
        let is_postgres = self.db.is_postgres();
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let username = normalize_request_log_filter(username);
        let search = normalize_request_log_filter(search);
        let txn = self
            .db
            .read()
            .begin_with_config(
                is_postgres.then_some(IsolationLevel::RepeatableRead),
                is_postgres.then_some(AccessMode::ReadOnly),
            )
            .await
            .map_err(|e| e.to_string())?;

        // Count query
        let mut count_sql = r#"SELECT COUNT(*) as cnt FROM request_logs rl
               WHERE 1 = 1"#
            .to_string();
        let mut count_values: Vec<SeaValue> = Vec::new();
        let mut count_idx = 1usize;
        append_request_log_filters(
            &mut count_sql,
            &mut count_values,
            &mut count_idx,
            is_postgres,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
            time_from,
            time_to,
        )?;
        let count_row = txn
            .query_one(self.db.stmt(&count_sql, count_values))
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = count_row
            .ok_or_else(|| "no count row".to_string())?
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;

        // Sum query
        let mut sum_sql = format!(
            "{} FROM request_logs rl WHERE 1 = 1",
            charge_aggregate_select(is_postgres)
        );
        let mut sum_values: Vec<SeaValue> = Vec::new();
        let mut sum_idx = 1usize;
        append_request_log_filters(
            &mut sum_sql,
            &mut sum_values,
            &mut sum_idx,
            is_postgres,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
            time_from,
            time_to,
        )?;
        let sum_row = txn
            .query_one(self.db.stmt(&sum_sql, sum_values))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no request log charge aggregate row".to_string())?;
        let total_charge_nano_usd = decode_charge_aggregate(&sum_row, is_postgres)?;

        // Rows query
        let mut rows_sql = r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.input_tokens, rl.output_tokens, rl.cache_read_tokens, rl.cache_creation_tokens,
                      rl.tool_prompt_tokens, rl.reasoning_tokens,
                      rl.accepted_prediction_tokens, rl.rejected_prediction_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms,
                      rl.request_ip, rl.reasoning_effort, rl.tried_providers_json, rl.request_kind,
                      rl.effective_provider_type, rl.affinity_hit, rl.affinity_key_hash, rl.affinity_target,
                      rl.session_affinity_value,
                      rl.created_at,
                      EXISTS (SELECT 1 FROM request_capture_records rcr WHERE rcr.request_id = rl.request_id AND rcr.user_id = rl.user_id) AS has_capture,
                      u.username AS username, ak.name AS api_key_name, ch.name AS channel_name, p.name AS provider_name
               FROM request_logs rl
               LEFT JOIN users u ON u.id = rl.user_id
               LEFT JOIN api_keys ak ON ak.id = rl.api_key_id
               LEFT JOIN monoize_channels ch ON ch.id = rl.channel_id
               LEFT JOIN monoize_providers p ON p.id = rl.provider_id
               WHERE 1 = 1"#
            .to_string();
        let mut rows_values: Vec<SeaValue> = Vec::new();
        let mut rows_idx = 1usize;
        append_request_log_filters(
            &mut rows_sql,
            &mut rows_values,
            &mut rows_idx,
            is_postgres,
            model.as_deref(),
            status.as_deref(),
            api_key_id.as_deref(),
            username.as_deref(),
            search.as_deref(),
            time_from,
            time_to,
        )?;
        if is_postgres {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC NULLS LAST, rl.created_at DESC, rl.id DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        } else {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC, rl.created_at DESC, rl.id DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        }
        rows_values.push(SeaValue::BigInt(Some(limit)));
        rows_values.push(SeaValue::BigInt(Some(offset)));

        let rows = txn
            .query_all(self.db.stmt(&rows_sql, rows_values))
            .await
            .map_err(|e| e.to_string())?;

        txn.commit().await.map_err(|e| e.to_string())?;
        let mut logs = rows
            .into_iter()
            .map(|row| row_to_request_log(&row))
            .collect::<Result<Vec<_>, _>>()?;
        self.enrich_request_log_tried_provider_names(&mut logs)
            .await?;

        Ok((logs, total, total_charge_nano_usd))
    }

    pub async fn get_dashboard_analytics(
        &self,
        user_id: Option<&str>,
        time_from: &str,
        time_to: &str,
        today_start: &str,
        bucket_count: i64,
    ) -> Result<DashboardAnalyticsRaw, String> {
        let is_sqlite = self.db.is_sqlite();
        let time_from_unix_ms = chrono::DateTime::parse_from_rfc3339(time_from)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let time_to_unix_ms = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let range_ms = time_to_unix_ms
            .checked_sub(time_from_unix_ms)
            .ok_or_else(|| "analytics time range overflow".to_string())?;
        if range_ms <= 0 || bucket_count <= 0 {
            return Err("analytics time range and bucket count must be positive".to_string());
        }

        let model_sql = analytics_model_bucket_sql(is_sqlite, user_id.is_some());
        let mut model_values: Vec<SeaValue> = vec![
            time_from_unix_ms.into(),
            bucket_count.into(),
            range_ms.into(),
            time_from_unix_ms.into(),
            time_to_unix_ms.into(),
        ];
        if let Some(uid) = user_id {
            model_values.push(uid.into());
        }

        let model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(&model_sql, model_values))
            .await
            .map_err(|e| e.to_string())?;

        let model_buckets = model_rows
            .into_iter()
            .map(|row| {
                let bucket_idx: i64 = row.try_get("", "bucket_idx").map_err(|e| e.to_string())?;
                let model = row.try_get("", "model").map_err(|e| e.to_string())?;
                let cost_nano = decode_charge_aggregate(&row, !is_sqlite)?
                    .parse::<i128>()
                    .map_err(|_| "request log charge aggregate overflow".to_string())?;
                let call_count = row.try_get("", "call_count").map_err(|e| e.to_string())?;
                Ok(AnalyticsModelBucketRow {
                    bucket_idx: bucket_idx.clamp(0, bucket_count - 1),
                    model,
                    cost_nano,
                    call_count,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let bucket_expr = analytics_bucket_expr(is_sqlite);

        // 2. Provider bucketed aggregation (calls only)
        let mut prov_sql = format!(
            r#"SELECT
                 {bucket_expr} AS bucket_idx,
                 COALESCE(mp.name, rl.provider_id, 'unknown') AS provider_label,
                 COUNT(*) AS call_count
                FROM request_logs rl
                LEFT JOIN monoize_providers mp ON rl.provider_id = mp.id
               WHERE {time_col} >= $4 AND {time_col} < $5"#,
            time_col = "rl.created_at_unix_ms"
        );
        prov_sql.push_str(" AND rl.created_at_unix_ms IS NOT NULL");
        let mut prov_values: Vec<SeaValue> = vec![
            time_from_unix_ms.into(),
            bucket_count.into(),
            range_ms.into(),
            time_from_unix_ms.into(),
            time_to_unix_ms.into(),
        ];
        let mut prov_idx = 6usize;

        if let Some(uid) = user_id {
            prov_sql.push_str(&format!(" AND rl.user_id = ${prov_idx}"));
            prov_values.push(uid.into());
            prov_idx += 1;
        }
        let _ = prov_idx;
        prov_sql.push_str(" GROUP BY bucket_idx, provider_label");

        let prov_rows = self
            .db
            .read()
            .query_all(self.db.stmt(&prov_sql, prov_values))
            .await
            .map_err(|e| e.to_string())?;

        let provider_buckets: Vec<AnalyticsProviderBucketRow> = prov_rows
            .into_iter()
            .map(|row| {
                let idx: i64 = row.try_get("", "bucket_idx").map_err(|e| e.to_string())?;
                Ok(AnalyticsProviderBucketRow {
                    bucket_idx: idx.clamp(0, bucket_count - 1),
                    provider_label: row
                        .try_get("", "provider_label")
                        .map_err(|e| e.to_string())?,
                    call_count: row.try_get("", "call_count").map_err(|e| e.to_string())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let (total_cost_nano_usd, total_calls) = model_buckets.iter().try_fold(
            (0i128, 0i64),
            |(cost, calls), row| -> Result<(i128, i64), String> {
                Ok((
                    cost.checked_add(row.cost_nano)
                        .ok_or_else(|| "analytics cost aggregate overflow".to_string())?,
                    calls
                        .checked_add(row.call_count)
                        .ok_or_else(|| "analytics call count overflow".to_string())?,
                ))
            },
        )?;

        let mut today_sql = format!(
            "{}, COUNT(*) AS call_count FROM request_logs rl WHERE rl.created_at_unix_ms >= $1 AND rl.created_at_unix_ms IS NOT NULL",
            charge_aggregate_select(!is_sqlite)
        );
        let today_start_unix_ms = chrono::DateTime::parse_from_rfc3339(today_start)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let mut today_values: Vec<SeaValue> = vec![today_start_unix_ms.into()];

        if let Some(uid) = user_id {
            today_sql.push_str(" AND rl.user_id = $2");
            today_values.push(uid.into());
        }
        let today_row = self
            .db
            .read()
            .query_one(self.db.stmt(&today_sql, today_values))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no today analytics aggregate row".to_string())?;
        let today_calls: i64 = today_row
            .try_get("", "call_count")
            .map_err(|e| e.to_string())?;
        let today_cost_nano_usd = decode_charge_aggregate(&today_row, !is_sqlite)?
            .parse::<i128>()
            .map_err(|_| "request log charge is outside the signed i128 domain".to_string())?;

        Ok(DashboardAnalyticsRaw {
            model_buckets,
            provider_buckets,
            total_cost_nano_usd,
            total_calls,
            today_cost_nano_usd,
            today_calls,
        })
    }

    pub async fn get_users_today_usage(
        &self,
        today_start: &str,
    ) -> Result<Vec<super::UserTodayUsage>, String> {
        let is_sqlite = self.db.is_sqlite();
        let today_start_unix_ms = chrono::DateTime::parse_from_rfc3339(today_start)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let charge_columns = charge_aggregate_columns(!is_sqlite);
        let sql = format!(
            "SELECT rl.user_id, {charge_columns}, COUNT(*) AS call_count \
             FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 \
               AND rl.created_at_unix_ms IS NOT NULL \
               AND rl.user_id IS NOT NULL \
             GROUP BY rl.user_id"
        );
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(&sql, vec![today_start_unix_ms.into()]))
            .await
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                let user_id: String = row.try_get("", "user_id").map_err(|e| e.to_string())?;
                let today_calls: i64 = row.try_get("", "call_count").map_err(|e| e.to_string())?;
                let today_cost_nano_usd = decode_charge_aggregate(&row, !is_sqlite)?
                    .parse::<i128>()
                    .map_err(|_| {
                        "request log charge is outside the signed i128 domain".to_string()
                    })?;
                Ok(super::UserTodayUsage {
                    user_id,
                    today_calls,
                    today_cost_nano_usd,
                })
            })
            .collect()
    }

    /// Admin dashboard usage ranking (admin-dashboard.spec.md AD-2/AD-5):
    /// per-user call count and charge aggregate over `[time_from, time_to)`,
    /// joined with usernames, ordered by cost desc / calls desc / username asc,
    /// limited to `limit` rows. Aggregation happens in SQL.
    pub async fn get_users_usage_ranking(
        &self,
        time_from: &str,
        time_to: &str,
        limit: i64,
    ) -> Result<Vec<super::UserUsageRankingRow>, String> {
        let is_sqlite = self.db.is_sqlite();
        let time_from_unix_ms = chrono::DateTime::parse_from_rfc3339(time_from)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let time_to_unix_ms = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        if time_from_unix_ms >= time_to_unix_ms {
            return Err("usage ranking time range must be positive".to_string());
        }
        let limit = limit.clamp(1, 20);
        let charge_columns = charge_aggregate_columns(!is_sqlite);
        let charge_order = charge_aggregate_order_expr(!is_sqlite, "ranked");
        let sql = format!(
            "SELECT * FROM ( \
                SELECT rl.user_id, u.username AS username, {charge_columns}, COUNT(*) AS call_count \
                FROM request_logs rl \
                LEFT JOIN users u ON u.id = rl.user_id \
                WHERE rl.created_at_unix_ms >= $1 \
                  AND rl.created_at_unix_ms < $2 \
                  AND rl.created_at_unix_ms IS NOT NULL \
                  AND rl.user_id IS NOT NULL \
                GROUP BY rl.user_id, u.username \
             ) ranked \
             ORDER BY {charge_order}, ranked.call_count DESC, ranked.username ASC \
             LIMIT $3"
        );
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &sql,
                vec![
                    time_from_unix_ms.into(),
                    time_to_unix_ms.into(),
                    limit.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                let user_id: String = row.try_get("", "user_id").map_err(|e| e.to_string())?;
                let username: Option<String> = row.try_get("", "username").ok();
                let call_count: i64 = row.try_get("", "call_count").map_err(|e| e.to_string())?;
                let cost_nano_usd = decode_charge_aggregate(&row, !is_sqlite)?
                    .parse::<i128>()
                    .map_err(|_| {
                        "request log charge is outside the signed i128 domain".to_string()
                    })?;
                Ok(super::UserUsageRankingRow {
                    user_id,
                    username,
                    call_count,
                    cost_nano_usd,
                })
            })
            .collect()
    }

    pub async fn get_channels_today_usage(
        &self,
        today_start: &str,
    ) -> Result<Vec<super::ChannelTodayUsage>, String> {
        let is_sqlite = self.db.is_sqlite();
        let today_start_unix_ms = chrono::DateTime::parse_from_rfc3339(today_start)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let charge_columns = charge_aggregate_columns(!is_sqlite);
        let sql = format!(
            "SELECT rl.channel_id, {charge_columns}, COUNT(*) AS call_count \
             FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 \
               AND rl.created_at_unix_ms IS NOT NULL \
               AND rl.channel_id IS NOT NULL \
             GROUP BY rl.channel_id"
        );
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(&sql, vec![today_start_unix_ms.into()]))
            .await
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                let channel_id: String =
                    row.try_get("", "channel_id").map_err(|e| e.to_string())?;
                let today_calls: i64 = row.try_get("", "call_count").map_err(|e| e.to_string())?;
                let today_cost_nano_usd = decode_charge_aggregate(&row, !is_sqlite)?
                    .parse::<i128>()
                    .map_err(|_| {
                        "request log charge is outside the signed i128 domain".to_string()
                    })?;
                Ok(super::ChannelTodayUsage {
                    channel_id,
                    today_calls,
                    today_cost_nano_usd,
                })
            })
            .collect()
    }

    pub async fn get_today_usage_totals(&self, today_start: &str) -> Result<(i64, i128), String> {
        let is_sqlite = self.db.is_sqlite();
        let today_start_unix_ms = chrono::DateTime::parse_from_rfc3339(today_start)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let sql = format!(
            "{}, COUNT(*) AS call_count FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 AND rl.created_at_unix_ms IS NOT NULL",
            charge_aggregate_select(!is_sqlite)
        );
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(&sql, vec![today_start_unix_ms.into()]))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no today usage aggregate row".to_string())?;
        let today_calls: i64 = row.try_get("", "call_count").map_err(|e| e.to_string())?;
        let today_cost_nano_usd = decode_charge_aggregate(&row, !is_sqlite)?
            .parse::<i128>()
            .map_err(|_| "request log charge is outside the signed i128 domain".to_string())?;
        Ok((today_calls, today_cost_nano_usd))
    }
}

#[cfg(test)]
mod today_usage_tests {
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::users::{UserRole, UserStore};
    use chrono::{NaiveTime, Utc};
    use sea_orm::ConnectionTrait;
    use sea_orm_migration::MigratorTrait;

    #[tokio::test]
    async fn groups_today_usage_by_user_and_ignores_prior_days() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        let store = UserStore::new(db.clone(), log_tx)
            .await
            .expect("store creates");
        let alice = store
            .create_user("alice_usage", "password12", UserRole::User, None)
            .await
            .expect("alice created");
        let bob = store
            .create_user("bob_usage", "password12", UserRole::User, None)
            .await
            .expect("bob created");

        let today_start = Utc::now().date_naive().and_time(NaiveTime::MIN).and_utc();
        let today_ms = today_start.timestamp_millis() + 3_600_000;
        let yesterday_ms = today_start.timestamp_millis() - 3_600_000;

        for (id, user_id, charge, created_ms) in [
            ("log-a1", alice.id.as_str(), "1000", today_ms),
            ("log-a2", alice.id.as_str(), "2500", today_ms + 1),
            ("log-a-old", alice.id.as_str(), "999999", yesterday_ms),
            ("log-b1", bob.id.as_str(), "7", today_ms),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (id, user_id, model, is_stream, status, created_at, created_at_unix_ms, charge_nano_usd) VALUES ($1, $2, 'm', 0, 'success', $3, $4, $5)",
                    vec![
                        id.into(),
                        user_id.into(),
                        today_start.to_rfc3339().into(),
                        created_ms.into(),
                        charge.into(),
                    ],
                ))
                .await
                .expect("log inserted");
        }

        let rows = store
            .get_users_today_usage(&today_start.to_rfc3339())
            .await
            .expect("usage query succeeds");
        let by_user: std::collections::BTreeMap<_, _> = rows
            .into_iter()
            .map(|row| (row.user_id, (row.today_calls, row.today_cost_nano_usd)))
            .collect();
        assert_eq!(by_user.get(&alice.id), Some(&(2, 3500)));
        assert_eq!(by_user.get(&bob.id), Some(&(1, 7)));
    }

    #[tokio::test]
    async fn request_log_lists_report_has_capture_from_metadata_records() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        let store = UserStore::new(db.clone(), log_tx)
            .await
            .expect("store creates");
        let alice = store
            .create_user("alice_capture", "password12", UserRole::User, None)
            .await
            .expect("alice created");

        let now_ms = Utc::now().timestamp_millis();
        for (id, request_id, created_ms) in [
            ("cap-log-1", "req-with-capture", now_ms),
            ("cap-log-2", "req-without-capture", now_ms + 1),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (id, request_id, user_id, model, is_stream, status, created_at, created_at_unix_ms, charge_nano_usd) VALUES ($1, $2, $3, 'm', 0, 'success', $4, $5, '0')",
                    vec![
                        id.into(),
                        request_id.into(),
                        alice.id.as_str().into(),
                        Utc::now().to_rfc3339().into(),
                        created_ms.into(),
                    ],
                ))
                .await
                .expect("log inserted");
        }
        // Matching record marks has_capture; a record owned by another user
        // for the same request_id must not (RCV-L2 matches on request_id AND
        // user_id).
        for (file_name, request_id, user_id) in [
            ("a.json", "req-with-capture", alice.id.as_str()),
            ("b.json", "req-without-capture", "someone-else"),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_capture_records (file_name, request_id, user_id, api_key_id, created_at, created_at_unix_ms, size_bytes) VALUES ($1, $2, $3, 'key-1', $4, $5, 10)",
                    vec![
                        file_name.into(),
                        request_id.into(),
                        user_id.into(),
                        Utc::now().to_rfc3339().into(),
                        now_ms.into(),
                    ],
                ))
                .await
                .expect("capture record inserted");
        }

        let (user_logs, _, _) = store
            .list_request_logs_by_user(&alice.id, 50, 0, None, None, None, None, None, None)
            .await
            .expect("user list succeeds");
        let by_request: std::collections::BTreeMap<_, _> = user_logs
            .iter()
            .map(|log| (log.request_id.clone().unwrap(), log.has_capture))
            .collect();
        assert_eq!(by_request.get("req-with-capture"), Some(&true));
        assert_eq!(by_request.get("req-without-capture"), Some(&false));

        let (all_logs, _, _) = store
            .list_all_request_logs(50, 0, None, None, None, None, None, None, None)
            .await
            .expect("admin list succeeds");
        let by_request_all: std::collections::BTreeMap<_, _> = all_logs
            .iter()
            .map(|log| (log.request_id.clone().unwrap(), log.has_capture))
            .collect();
        assert_eq!(by_request_all.get("req-with-capture"), Some(&true));
        assert_eq!(by_request_all.get("req-without-capture"), Some(&false));
    }

    #[tokio::test]
    async fn usage_ranking_orders_by_cost_desc_and_joins_usernames() {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }
        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        let store = UserStore::new(db.clone(), log_tx)
            .await
            .expect("store creates");
        let alice = store
            .create_user("alice_rank", "password12", UserRole::User, None)
            .await
            .expect("alice created");
        let bob = store
            .create_user("bob_rank", "password12", UserRole::User, None)
            .await
            .expect("bob created");

        let window_from = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        let window_to = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let now_ms = Utc::now().timestamp_millis();

        for (id, user_id, charge, created_ms) in [
            ("rank-a1", alice.id.as_str(), "2500", now_ms),
            ("rank-a2", alice.id.as_str(), "1000", now_ms + 1),
            ("rank-b1", bob.id.as_str(), "9000", now_ms + 2),
        ] {
            db.write()
                .await
                .execute(db.stmt(
                    "INSERT INTO request_logs (id, user_id, model, is_stream, status, created_at, created_at_unix_ms, charge_nano_usd) VALUES ($1, $2, 'm', 0, 'success', $3, $4, $5)",
                    vec![
                        id.into(),
                        user_id.into(),
                        Utc::now().to_rfc3339().into(),
                        created_ms.into(),
                        charge.into(),
                    ],
                ))
                .await
                .expect("log inserted");
        }

        let rows = store
            .get_users_usage_ranking(&window_from, &window_to, 20)
            .await
            .expect("ranking query succeeds");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].user_id, bob.id);
        assert_eq!(rows[0].username.as_deref(), Some("bob_rank"));
        assert_eq!(rows[0].call_count, 1);
        assert_eq!(rows[0].cost_nano_usd, 9000);
        assert_eq!(rows[1].user_id, alice.id);
        assert_eq!(rows[1].call_count, 2);
        assert_eq!(rows[1].cost_nano_usd, 3500);
    }
}

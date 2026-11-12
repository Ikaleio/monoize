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
const REQUEST_LOG_RETENTION_DELETE_BATCH_ROWS: u64 = 5000;
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
        if !json_nonempty_str(obj.get("provider_name"))
            && let Some(id) = obj.get("provider_id").and_then(Value::as_str)
            && let Some(name) = provider_names.get(id)
        {
            obj.insert("provider_name".into(), Value::String(name.clone()));
        }
        if !json_nonempty_str(obj.get("channel_name"))
            && let Some(id) = obj.get("channel_id").and_then(Value::as_str)
            && let Some(name) = channel_names.get(id)
        {
            obj.insert("channel_name".into(), Value::String(name.clone()));
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

fn enrich_request_log_page_sql(page_sql: &str, is_postgres: bool, next_param: usize) -> String {
    let nulls_last = if is_postgres { " NULLS LAST" } else { "" };
    let order = format!("rl.created_at_unix_ms DESC{nulls_last}, rl.created_at DESC, rl.id DESC");
    // LIMIT inside the derived table bounds enrichment work independently of join planning.
    format!(
        "SELECT rl.*, \
         EXISTS (SELECT 1 FROM request_capture_records rcr WHERE rcr.request_id = rl.request_id AND rcr.user_id = rl.user_id) AS has_capture, \
         u.username AS username, ak.name AS api_key_name, ch.name AS channel_name, p.name AS provider_name \
         FROM ({page_sql} ORDER BY {order} LIMIT ${next_param} OFFSET ${offset_param}) rl \
         LEFT JOIN users u ON u.id = rl.user_id \
         LEFT JOIN api_keys ak ON ak.id = rl.api_key_id \
         LEFT JOIN monoize_channels ch ON ch.id = rl.channel_id \
         LEFT JOIN monoize_providers p ON p.id = rl.provider_id \
         ORDER BY {order}",
        offset_param = next_param + 1,
    )
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

fn request_log_list_aggregates_select(is_postgres: bool) -> String {
    format!(
        "SELECT COUNT(*) AS cnt, {}",
        charge_aggregate_columns(is_postgres)
    )
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

fn analytics_token_sum_expr() -> &'static str {
    // CAST to BIGINT so PostgreSQL SUM(bigint) (NUMERIC) and SQLite SUM
    // (INTEGER) decode identically into i64, mirroring get_user_live_usage.
    "CAST(SUM(\
        COALESCE(rl.input_tokens, 0) + COALESCE(rl.output_tokens, 0) + \
        COALESCE(rl.cache_read_tokens, 0) + COALESCE(rl.cache_creation_tokens, 0) + \
        COALESCE(rl.reasoning_tokens, 0)\
     ) AS BIGINT) AS token_count"
}

fn append_performance_target_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    idx: &mut usize,
    provider_ids: Option<&[String]>,
    model: Option<&str>,
) {
    if let Some(ids) = provider_ids {
        sql.push_str(" AND rl.provider_id IN (");
        for (i, id) in ids.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&format!("${}", *idx));
            values.push(id.clone().into());
            *idx += 1;
        }
        sql.push(')');
    }
    if let Some(model) = model {
        sql.push_str(&format!(" AND rl.model = ${}", *idx));
        values.push(model.to_string().into());
        *idx += 1;
    }
}

fn analytics_model_bucket_sql(is_sqlite: bool, user_scoped: bool) -> String {
    let bucket_expr = analytics_bucket_expr(is_sqlite);
    let charge_columns = charge_aggregate_columns(!is_sqlite);
    let token_sum = analytics_token_sum_expr();
    let user_filter = if user_scoped {
        " AND rl.user_id = $6"
    } else {
        ""
    };
    format!(
        "SELECT {bucket_expr} AS bucket_idx, rl.model, {charge_columns}, COUNT(*) AS call_count, \
         {token_sum}, \
         CAST(SUM(COALESCE(rl.input_tokens, 0)) AS BIGINT) AS input_token_count, \
         CAST(SUM(COALESCE(rl.cache_read_tokens, 0)) AS BIGINT) AS cache_read_token_count \
         FROM request_logs rl \
         WHERE rl.created_at_unix_ms >= $4 AND rl.created_at_unix_ms < $5{user_filter} \
         GROUP BY bucket_idx, rl.model \
         ORDER BY bucket_idx, rl.model"
    )
}

fn append_request_log_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    idx: &mut usize,
    is_postgres: bool,
    filters: RequestLogFilters<'_>,
) -> Result<(), String> {
    let RequestLogFilters {
        model,
        status,
        api_key_id,
        channel_id,
        username,
        search,
        time_from,
        time_to,
    } = filters;
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
    if let Some(channel_id) = channel_id {
        sql.push_str(&format!(" AND rl.channel_id = ${}", *idx));
        values.push(channel_id.into());
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
        upstream_response_model: request_log_row_value(row, "upstream_response_model")?,
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

/// Optional predicates shared by request-log page and aggregate queries.
#[derive(Clone, Copy, Default)]
pub struct RequestLogFilters<'a> {
    pub model: Option<&'a str>,
    pub status: Option<&'a str>,
    pub api_key_id: Option<&'a str>,
    pub channel_id: Option<&'a str>,
    pub username: Option<&'a str>,
    pub search: Option<&'a str>,
    pub time_from: Option<&'a str>,
    pub time_to: Option<&'a str>,
}

mod analytics;
mod lifecycle;
mod listing;
mod usage;

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
         {token_sum} \
         FROM request_logs rl \
         WHERE rl.created_at_unix_ms >= $4 AND rl.created_at_unix_ms < $5{user_filter} \
         GROUP BY bucket_idx, rl.model \
         ORDER BY bucket_idx, rl.model"
    )
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
        self.cleanup_expired_request_logs_batched(
            cutoff_unix_ms,
            REQUEST_LOG_RETENTION_DELETE_BATCH_ROWS,
        )
        .await
    }

    /// RL-S9a: delete expired rows in bounded batches. Each batch is one
    /// autocommitted DELETE, so the writer (the SQLite write lock, Postgres
    /// row locks and WAL volume) is held for at most `batch_rows` rows at a
    /// time and request-log flushes can interleave between batches.
    async fn cleanup_expired_request_logs_batched(
        &self,
        cutoff_unix_ms: i64,
        batch_rows: u64,
    ) -> Result<u64, String> {
        let sql = format!(
            "DELETE FROM request_logs WHERE id IN (\
             SELECT id FROM request_logs \
             WHERE created_at_unix_ms IS NOT NULL AND created_at_unix_ms < $1 \
             LIMIT {batch_rows})"
        );
        let mut total_deleted = 0u64;
        loop {
            let result = self
                .db
                .write()
                .await
                .execute(self.db.stmt(&sql, vec![cutoff_unix_ms.into()]))
                .await
                .map_err(|e| e.to_string())?;
            let deleted = result.rows_affected();
            total_deleted += deleted;
            if deleted < batch_rows {
                return Ok(total_deleted);
            }
        }
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
                let token_count: i64 = row
                    .try_get::<Option<i64>>("", "token_count")
                    .map_err(|e| e.to_string())?
                    .unwrap_or(0);
                Ok(AnalyticsModelBucketRow {
                    bucket_idx: bucket_idx.clamp(0, bucket_count - 1),
                    model,
                    cost_nano,
                    call_count,
                    token_count,
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

    /// Global (not user-scoped) performance aggregates for one dashboard target
    /// over `[time_from_unix_ms, time_to_unix_ms)` split into `brick_count`
    /// equal-width hour bricks (DH-9b).
    pub async fn get_performance_target_stats(
        &self,
        time_from_unix_ms: i64,
        time_to_unix_ms: i64,
        brick_count: i64,
        provider_ids: Option<&[String]>,
        model: Option<&str>,
    ) -> Result<super::PerformanceTargetRaw, String> {
        if brick_count <= 0 || time_to_unix_ms <= time_from_unix_ms {
            return Err("performance window and brick count must be positive".to_string());
        }
        if let Some(ids) = provider_ids
            && ids.is_empty()
        {
            return Ok(super::PerformanceTargetRaw {
                hour_buckets: Vec::new(),
                avg_ttft_ms: None,
                avg_tps: None,
            });
        }

        let is_sqlite = self.db.is_sqlite();
        let range_ms = time_to_unix_ms
            .checked_sub(time_from_unix_ms)
            .ok_or_else(|| "performance time range overflow".to_string())?;
        let hour_expr = if is_sqlite {
            "CAST(((rl.created_at_unix_ms - $1) * $3) / $4 AS BIGINT)"
        } else {
            "FLOOR(((rl.created_at_unix_ms - $1)::NUMERIC * $3) / $4)::BIGINT"
        };

        let mut hour_sql = format!(
            "SELECT {hour_expr} AS hour_idx, \
             COUNT(*) AS finished_count, \
             SUM(CASE WHEN rl.status IN ('success', 'client_gone') THEN 1 ELSE 0 END) \
                 AS success_count \
             FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 AND rl.created_at_unix_ms < $2 \
               AND rl.status <> 'pending'"
        );
        let mut values: Vec<SeaValue> = vec![
            time_from_unix_ms.into(),
            time_to_unix_ms.into(),
            brick_count.into(),
            range_ms.into(),
        ];
        let mut idx = 5usize;
        append_performance_target_filters(
            &mut hour_sql,
            &mut values,
            &mut idx,
            provider_ids,
            model,
        );
        hour_sql.push_str(" GROUP BY hour_idx");

        let hour_rows = self
            .db
            .read()
            .query_all(self.db.stmt(&hour_sql, values))
            .await
            .map_err(|e| e.to_string())?;

        let hour_buckets = hour_rows
            .into_iter()
            .map(|row| {
                let hour_idx: i64 = row.try_get("", "hour_idx").map_err(|e| e.to_string())?;
                Ok(super::PerformanceHourBucketRow {
                    hour_idx: hour_idx.clamp(0, brick_count - 1),
                    finished_count: row
                        .try_get("", "finished_count")
                        .map_err(|e| e.to_string())?,
                    success_count: row
                        .try_get::<Option<i64>>("", "success_count")
                        .map_err(|e| e.to_string())?
                        .unwrap_or(0),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        // FL4a / frontend computeTps: stream+ttfb uses duration-ttfb window;
        // otherwise duration. Numerator is output_tokens.
        let tps_expr = "CASE \
            WHEN COALESCE(rl.output_tokens, 0) > 0 \
                 AND (CASE \
                        WHEN rl.is_stream = 1 \
                             AND rl.duration_ms IS NOT NULL \
                             AND rl.ttfb_ms IS NOT NULL \
                             AND rl.duration_ms > rl.ttfb_ms \
                          THEN rl.duration_ms - rl.ttfb_ms \
                        ELSE rl.duration_ms \
                      END) > 0 \
            THEN (CAST(COALESCE(rl.output_tokens, 0) AS REAL) * 1000.0) \
                 / CAST( \
                     CASE \
                       WHEN rl.is_stream = 1 \
                            AND rl.duration_ms IS NOT NULL \
                            AND rl.ttfb_ms IS NOT NULL \
                            AND rl.duration_ms > rl.ttfb_ms \
                         THEN rl.duration_ms - rl.ttfb_ms \
                       ELSE rl.duration_ms \
                     END AS REAL \
                   ) \
            ELSE NULL \
          END";

        let mut avg_sql = format!(
            "SELECT \
               AVG(CASE WHEN rl.ttfb_ms IS NOT NULL AND rl.ttfb_ms > 0 \
                        THEN CAST(rl.ttfb_ms AS REAL) ELSE NULL END) AS avg_ttft_ms, \
               AVG({tps_expr}) AS avg_tps \
             FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 AND rl.created_at_unix_ms < $2 \
               AND rl.status <> 'pending'"
        );
        let mut avg_values: Vec<SeaValue> = vec![time_from_unix_ms.into(), time_to_unix_ms.into()];
        let mut avg_idx = 3usize;
        append_performance_target_filters(
            &mut avg_sql,
            &mut avg_values,
            &mut avg_idx,
            provider_ids,
            model,
        );

        let avg_row = self
            .db
            .read()
            .query_one(self.db.stmt(&avg_sql, avg_values))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no performance average row".to_string())?;

        let avg_ttft_ms: Option<f64> = avg_row
            .try_get::<Option<f64>>("", "avg_ttft_ms")
            .map_err(|e| e.to_string())?;
        let avg_tps: Option<f64> = avg_row
            .try_get::<Option<f64>>("", "avg_tps")
            .map_err(|e| e.to_string())?;

        Ok(super::PerformanceTargetRaw {
            hour_buckets,
            avg_ttft_ms,
            avg_tps,
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

    /// Rolling 60-second per-user usage aggregate
    /// (`user-live-usage.spec.md` LU-3 through LU-8).
    ///
    /// One SQL statement over `[now - 60s, now)` on `created_at_unix_ms`,
    /// scoped to `user_id`. Token SUMs are cast to BIGINT so SQLite (INTEGER)
    /// and PostgreSQL (NUMERIC from SUM(bigint)) decode identically into i64;
    /// the cast wraps the aggregate, not the indexed range column (RL-S2b).
    pub async fn get_user_live_usage(&self, user_id: &str) -> Result<super::UserLiveUsage, String> {
        let now_ms = Utc::now().timestamp_millis();
        let from_ms = now_ms - super::LIVE_USAGE_WINDOW_SECONDS * 1000;
        let sql = "SELECT COUNT(*) AS rpm, \
             CAST(COALESCE(SUM(COALESCE(rl.input_tokens, 0)), 0) AS BIGINT) AS input_tokens, \
             CAST(COALESCE(SUM(COALESCE(rl.output_tokens, 0)), 0) AS BIGINT) AS output_tokens, \
             CAST(COALESCE(SUM(COALESCE(rl.cache_read_tokens, 0)), 0) AS BIGINT) AS cache_read_tokens \
             FROM request_logs rl \
             WHERE rl.user_id = $1 \
               AND rl.created_at_unix_ms IS NOT NULL \
               AND rl.created_at_unix_ms >= $2 \
               AND rl.created_at_unix_ms < $3";
        let row = self
            .db
            .read()
            .query_one(
                self.db
                    .stmt(sql, vec![user_id.into(), from_ms.into(), now_ms.into()]),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no live usage aggregate row".to_string())?;
        Ok(super::UserLiveUsage {
            rpm: row.try_get("", "rpm").map_err(|e| e.to_string())?,
            input_tokens: row.try_get("", "input_tokens").map_err(|e| e.to_string())?,
            output_tokens: row
                .try_get("", "output_tokens")
                .map_err(|e| e.to_string())?,
            cache_read_tokens: row
                .try_get("", "cache_read_tokens")
                .map_err(|e| e.to_string())?,
        })
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

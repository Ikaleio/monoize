use super::{
    AnalyticsModelBucketRow, AnalyticsProviderBucketRow, DashboardAnalyticsRaw, InsertRequestLog,
    RequestLogApiKey, RequestLogBilling, RequestLogChannel, RequestLogError, RequestLogProvider,
    RequestLogRow, RequestLogTiming, RequestLogTokens, RequestLogUser, UserStore,
};
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use sea_orm::ConnectionTrait;
use sea_orm::Value as SeaValue;
use serde_json::Value;

const REQUEST_LOG_RETENTION_DAYS: i64 = 90;
pub(super) const REQUEST_LOG_RETENTION_INTERVAL_SECS: u64 = 3600;

fn normalize_request_log_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_optional_json_text(value: Option<String>) -> Option<Value> {
    value.and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
}

fn parse_optional_charge_decimal(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        Decimal::from_str_exact(trimmed)
            .ok()
            .map(|v| v.trunc().to_string())
    })
}

fn decimal_to_nano_i64(value: Decimal) -> Option<i64> {
    value.trunc().to_i64()
}

fn row_decimal_to_string(row: &sea_orm::QueryResult, col: &str) -> Option<String> {
    row.try_get::<Option<Decimal>>("", col)
        .ok()
        .flatten()
        .map(|v| v.trunc().to_string())
        .or_else(|| {
            parse_optional_charge_decimal(row.try_get::<Option<String>>("", col).unwrap_or(None))
        })
}

fn row_decimal_to_nano_i64(row: &sea_orm::QueryResult, col: &str) -> i64 {
    row.try_get::<Option<Decimal>>("", col)
        .ok()
        .flatten()
        .and_then(decimal_to_nano_i64)
        .or_else(|| {
            parse_optional_charge_decimal(row.try_get::<Option<String>>("", col).unwrap_or(None))
                .and_then(|v| decimal_to_nano_i64(Decimal::from_str_exact(&v).ok()?))
        })
        .unwrap_or(0)
}

fn postgres_charge_expr(column: &str) -> String {
    format!(
        "CASE WHEN {column} IS NULL OR btrim({column}) = '' THEN NULL WHEN {column} ~ '^-?[0-9]+$' THEN CAST({column} AS NUMERIC(39,0)) ELSE NULL END"
    )
}

fn request_log_time_filter_column() -> &'static str {
    "COALESCE(rl.created_at_unix_ms, -9223372036854775808)"
}

fn parse_decimal_query_to_i64(value: Option<String>) -> i64 {
    value
        .and_then(|v| {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                return None;
            }
            Decimal::from_str_exact(trimmed)
                .ok()
                .and_then(decimal_to_nano_i64)
        })
        .unwrap_or(0)
}

fn row_optional_i64(row: &sea_orm::QueryResult, col: &str) -> Option<i64> {
    row.try_get::<Option<i64>>("", col)
        .ok()
        .flatten()
        .or_else(|| {
            row.try_get::<Option<i32>>("", col)
                .ok()
                .flatten()
                .map(i64::from)
        })
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
) {
    if let Some(model) = model {
        let models: Vec<&str> = model
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if models.len() == 1 {
            sql.push_str(&format!(" AND rl.model LIKE '%' || ${} || '%'", *idx));
            values.push(models[0].into());
            *idx += 1;
        } else if !models.is_empty() {
            sql.push_str(" AND (");
            for (i, m) in models.iter().enumerate() {
                if i > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str(&format!("rl.model LIKE '%' || ${} || '%'", *idx));
                values.push((*m).into());
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
        let search_like = format!("%{search}%");
        sql.push_str(&format!(
            " AND (rl.model LIKE ${i} OR rl.upstream_model LIKE ${j} OR rl.request_id LIKE ${k} OR rl.request_ip LIKE ${l})",
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
            .map_err(|e| e.to_string())
            .ok()
            .map(|dt| dt.timestamp_millis());
        if let Some(time_from_unix_ms) = parsed {
            let _ = is_postgres;
            sql.push_str(&format!(
                " AND {} >= ${}",
                request_log_time_filter_column(),
                *idx
            ));
            values.push(time_from_unix_ms.into());
        }
        *idx += 1;
    }
    if let Some(time_to) = time_to {
        let parsed = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|e| e.to_string())
            .ok()
            .map(|dt| dt.timestamp_millis());
        if let Some(time_to_unix_ms) = parsed {
            let _ = is_postgres;
            sql.push_str(&format!(
                " AND {} < ${}",
                request_log_time_filter_column(),
                *idx
            ));
            values.push(time_to_unix_ms.into());
        }
        *idx += 1;
    }
}

fn row_to_request_log(row: &sea_orm::QueryResult) -> RequestLogRow {
    let is_stream = row.try_get::<i32>("", "is_stream").unwrap_or_else(|_| {
        row.try_get::<Option<i32>>("", "is_stream")
            .unwrap_or(None)
            .unwrap_or(0)
    }) == 1;

    let charge_nano_usd = row
        .try_get::<Option<String>>("", "charge_nano_usd")
        .unwrap_or(None);

    RequestLogRow {
        id: row.try_get("", "id").unwrap_or_default(),
        request_id: row.try_get("", "request_id").unwrap_or(None),
        created_at: row.try_get("", "created_at").unwrap_or_default(),
        status: row
            .try_get("", "status")
            .unwrap_or_else(|_| "unknown".to_string()),
        is_stream,
        model: row.try_get("", "model").unwrap_or_default(),
        upstream_model: row.try_get("", "upstream_model").unwrap_or(None),
        request_kind: row.try_get("", "request_kind").unwrap_or(None),
        reasoning_effort: row.try_get("", "reasoning_effort").unwrap_or(None),
        request_ip: row.try_get("", "request_ip").unwrap_or(None),
        tried_providers: parse_optional_json_text(
            row.try_get::<Option<String>>("", "tried_providers_json")
                .unwrap_or(None),
        ),
        provider: RequestLogProvider {
            id: row.try_get("", "provider_id").unwrap_or(None),
            name: row.try_get("", "provider_name").unwrap_or(None),
            multiplier: row.try_get("", "provider_multiplier").unwrap_or(None),
        },
        channel: RequestLogChannel {
            id: row.try_get("", "channel_id").unwrap_or(None),
            name: row.try_get("", "channel_name").unwrap_or(None),
        },
        user: RequestLogUser {
            id: row.try_get("", "user_id").unwrap_or_default(),
            username: row.try_get("", "username").unwrap_or(None),
        },
        api_key: RequestLogApiKey {
            id: row.try_get("", "api_key_id").unwrap_or(None),
            name: row.try_get("", "api_key_name").unwrap_or(None),
        },
        tokens: RequestLogTokens {
            input: row_optional_i64(row, "input_tokens"),
            output: row_optional_i64(row, "output_tokens"),
            cache_read: row_optional_i64(row, "cache_read_tokens"),
            cache_creation: row_optional_i64(row, "cache_creation_tokens"),
            tool_prompt: row_optional_i64(row, "tool_prompt_tokens"),
            reasoning: row_optional_i64(row, "reasoning_tokens"),
            accepted_prediction: row_optional_i64(row, "accepted_prediction_tokens"),
            rejected_prediction: row_optional_i64(row, "rejected_prediction_tokens"),
        },
        timing: {
            let duration_ms = row_optional_i64(row, "duration_ms");
            let ttfb_ms = row_optional_i64(row, "ttfb_ms");
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
                row.try_get::<Option<String>>("", "billing_breakdown_json")
                    .unwrap_or(None),
            ),
        },
        usage: parse_optional_json_text(
            row.try_get::<Option<String>>("", "usage_breakdown_json")
                .unwrap_or(None),
        ),
        error: RequestLogError {
            code: row.try_get("", "error_code").unwrap_or(None),
            message: row.try_get("", "error_message").unwrap_or(None),
            http_status: row_optional_i64(row, "error_http_status"),
        },
    }
}

impl UserStore {
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
        _provider_multiplier: f64,
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
        self.request_log_batcher.push(log).await;
        Ok(())
    }

    pub async fn insert_request_log(&self, log: InsertRequestLog) -> Result<(), String> {
        self.request_log_batcher.push(log).await;
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
        let is_postgres = self.db.is_postgres();
        let postgres_charge = postgres_charge_expr("rl.charge_nano_usd");

        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let search = normalize_request_log_filter(search);

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
        );
        let count_row = self
            .db
            .read()
            .query_one(self.db.stmt(&count_sql, count_values))
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = count_row
            .ok_or_else(|| "no count row".to_string())?
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;

        // Sum query
        let mut sum_sql = if is_postgres {
            format!(
                "SELECT COALESCE(SUM(COALESCE({postgres_charge}, 0)), 0) as total_charge FROM request_logs rl WHERE rl.user_id = $1"
            )
        } else {
            "SELECT CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) as total_charge FROM request_logs rl WHERE rl.user_id = $1".to_string()
        };
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
        );
        let sum_row = self
            .db
            .read()
            .query_one(self.db.stmt(&sum_sql, sum_values))
            .await
            .map_err(|e| e.to_string())?;
        let total_charge_row = sum_row.ok_or_else(|| "no sum row".to_string())?;
        let total_charge_nano_usd = if is_postgres {
            row_decimal_to_string(&total_charge_row, "total_charge")
                .unwrap_or_else(|| "0".to_string())
        } else {
            total_charge_row
                .try_get::<i64>("", "total_charge")
                .map(|v| v.to_string())
                .map_err(|e| e.to_string())?
        };

        // Rows query
        let mut rows_sql = format!(
            r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.input_tokens, rl.output_tokens, rl.cache_read_tokens, rl.cache_creation_tokens,
                      rl.tool_prompt_tokens, rl.reasoning_tokens,
                      rl.accepted_prediction_tokens, rl.rejected_prediction_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms, rl.request_ip, rl.reasoning_effort, rl.request_kind, rl.created_at
               FROM request_logs rl
               WHERE rl.user_id = $1"#
        );
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
        );
        if is_postgres {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC NULLS LAST, rl.created_at DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        } else {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC, rl.created_at DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        }
        rows_values.push(SeaValue::BigInt(Some(limit)));
        rows_values.push(SeaValue::BigInt(Some(offset)));

        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(&rows_sql, rows_values))
            .await
            .map_err(|e| e.to_string())?;

        let logs = rows
            .into_iter()
            .map(|row| row_to_request_log(&row))
            .collect();

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
        let is_postgres = self.db.is_postgres();
        let postgres_charge = postgres_charge_expr("rl.charge_nano_usd");

        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let username = normalize_request_log_filter(username);
        let search = normalize_request_log_filter(search);

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
        );
        let count_row = self
            .db
            .read()
            .query_one(self.db.stmt(&count_sql, count_values))
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = count_row
            .ok_or_else(|| "no count row".to_string())?
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;

        // Sum query
        let mut sum_sql = if is_postgres {
            format!(
                "SELECT COALESCE(SUM(COALESCE({postgres_charge}, 0)), 0) as total_charge FROM request_logs rl WHERE 1 = 1"
            )
        } else {
            r#"SELECT CAST(COALESCE(SUM(CAST(rl.charge_nano_usd AS BIGINT)), 0) AS BIGINT) as total_charge FROM request_logs rl
               WHERE 1 = 1"#.to_string()
        };
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
        );
        let sum_row = self
            .db
            .read()
            .query_one(self.db.stmt(&sum_sql, sum_values))
            .await
            .map_err(|e| e.to_string())?;
        let total_charge_row = sum_row.ok_or_else(|| "no sum row".to_string())?;
        let total_charge_nano_usd = if is_postgres {
            row_decimal_to_string(&total_charge_row, "total_charge")
                .unwrap_or_else(|| "0".to_string())
        } else {
            total_charge_row
                .try_get::<i64>("", "total_charge")
                .map(|v| v.to_string())
                .map_err(|e| e.to_string())?
        };

        // Rows query
        let mut rows_sql = format!(
            r#"SELECT rl.id, rl.request_id, rl.user_id, rl.api_key_id, rl.model, rl.provider_id, rl.upstream_model,
                      rl.channel_id, rl.is_stream,
                      rl.input_tokens, rl.output_tokens, rl.cache_read_tokens, rl.cache_creation_tokens,
                      rl.tool_prompt_tokens, rl.reasoning_tokens,
                      rl.accepted_prediction_tokens, rl.rejected_prediction_tokens,
                      rl.provider_multiplier, rl.charge_nano_usd, rl.status,
                      rl.usage_breakdown_json, rl.billing_breakdown_json,
                      rl.error_code, rl.error_message, rl.error_http_status,
                      rl.duration_ms, rl.ttfb_ms, rl.request_ip, rl.reasoning_effort, rl.request_kind, rl.created_at
               FROM request_logs rl
               WHERE 1 = 1"#
        );
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
        );
        if is_postgres {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC NULLS LAST, rl.created_at DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        } else {
            rows_sql.push_str(&format!(
                " ORDER BY rl.created_at_unix_ms DESC, rl.created_at DESC LIMIT ${} OFFSET ${}",
                rows_idx,
                rows_idx + 1
            ));
        }
        rows_values.push(SeaValue::BigInt(Some(limit)));
        rows_values.push(SeaValue::BigInt(Some(offset)));

        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(&rows_sql, rows_values))
            .await
            .map_err(|e| e.to_string())?;

        let logs = rows
            .into_iter()
            .map(|row| row_to_request_log(&row))
            .collect();

        Ok((logs, total, total_charge_nano_usd))
    }

    pub async fn get_dashboard_analytics(
        &self,
        user_id: Option<&str>,
        time_from: &str,
        time_to: &str,
        today_start: &str,
        bucket_count: i64,
        bucket_width_days: f64,
    ) -> Result<DashboardAnalyticsRaw, String> {
        let is_sqlite = self.db.is_sqlite();
        let charge_expr = if is_sqlite {
            "CAST(rl.charge_nano_usd AS BIGINT)"
        } else {
            "CASE WHEN rl.charge_nano_usd IS NULL OR btrim(rl.charge_nano_usd) = '' THEN NULL WHEN rl.charge_nano_usd ~ '^-?[0-9]+$' THEN CAST(rl.charge_nano_usd AS NUMERIC(39,0)) ELSE NULL END"
        };

        // 1. Model bucketed aggregation (cost + calls)
        let bucket_expr = if is_sqlite {
            "CAST(((rl.created_at_unix_ms - $1) / 86400000.0) / $2 AS BIGINT)".to_string()
        } else {
            "CAST(((rl.created_at_unix_ms - $1)::DOUBLE PRECISION / 86400000.0) / $2 AS BIGINT)"
                .to_string()
        };

        let mut model_sql = format!(
            r#"SELECT
                 {bucket_expr} AS bucket_idx,
                 rl.model,
                 COALESCE(SUM({charge_expr}), 0) AS cost_nano,
                 COUNT(*) AS call_count
               FROM request_logs rl
               WHERE {time_col} >= $3 AND {time_col} < $4"#,
            time_col = if is_sqlite {
                "rl.created_at_unix_ms".to_string()
            } else {
                "rl.created_at_unix_ms".to_string()
            }
        );
        model_sql.push_str(" AND rl.created_at_unix_ms IS NOT NULL");
        let time_from_unix_ms = chrono::DateTime::parse_from_rfc3339(time_from)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let time_to_unix_ms = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let mut model_values: Vec<SeaValue> = vec![
            time_from_unix_ms.into(),
            SeaValue::Double(Some(bucket_width_days)),
            time_from_unix_ms.into(),
            time_to_unix_ms.into(),
        ];
        let mut model_idx = 5usize;

        if let Some(uid) = user_id {
            model_sql.push_str(&format!(" AND rl.user_id = ${model_idx}"));
            model_values.push(uid.into());
            model_idx += 1;
        }
        let _ = model_idx;
        model_sql.push_str(" GROUP BY bucket_idx, rl.model");

        let model_rows = self
            .db
            .read()
            .query_all(self.db.stmt(&model_sql, model_values))
            .await
            .map_err(|e| e.to_string())?;

        let model_buckets: Vec<AnalyticsModelBucketRow> = model_rows
            .into_iter()
            .map(|row| {
                let idx: i64 = row.try_get("", "bucket_idx").unwrap_or(0);
                let cost_nano = if is_sqlite {
                    row.try_get::<i64>("", "cost_nano").unwrap_or(0)
                } else {
                    row_decimal_to_nano_i64(&row, "cost_nano")
                };
                AnalyticsModelBucketRow {
                    bucket_idx: idx.clamp(0, bucket_count - 1),
                    model: row.try_get("", "model").unwrap_or_default(),
                    cost_nano,
                    call_count: row.try_get("", "call_count").unwrap_or(0),
                }
            })
            .collect();

        // 2. Provider bucketed aggregation (calls only)
        let mut prov_sql = format!(
            r#"SELECT
                 {bucket_expr} AS bucket_idx,
                 COALESCE(mp.name, rl.provider_id, 'unknown') AS provider_label,
                 COUNT(*) AS call_count
               FROM request_logs rl
               LEFT JOIN monoize_providers mp ON rl.provider_id = mp.id
               WHERE {time_col} >= $3 AND {time_col} < $4"#,
            time_col = if is_sqlite {
                "rl.created_at_unix_ms".to_string()
            } else {
                "rl.created_at_unix_ms".to_string()
            }
        );
        prov_sql.push_str(" AND rl.created_at_unix_ms IS NOT NULL");
        let mut prov_values: Vec<SeaValue> = vec![
            time_from_unix_ms.into(),
            SeaValue::Double(Some(bucket_width_days)),
            time_from_unix_ms.into(),
            time_to_unix_ms.into(),
        ];
        let mut prov_idx = 5usize;

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
                let idx: i64 = row.try_get("", "bucket_idx").unwrap_or(0);
                AnalyticsProviderBucketRow {
                    bucket_idx: idx.clamp(0, bucket_count - 1),
                    provider_label: row.try_get("", "provider_label").unwrap_or_default(),
                    call_count: row.try_get("", "call_count").unwrap_or(0),
                }
            })
            .collect();

        // 3. Total stats for the range
        let mut total_sql = format!(
            r#"SELECT
                 COALESCE(SUM({charge_expr}), 0) AS total_cost,
                 COUNT(*) AS total_calls
               FROM request_logs rl
               WHERE {time_col} >= $1 AND {time_col} < $2"#,
            time_col = if is_sqlite {
                "rl.created_at_unix_ms".to_string()
            } else {
                "rl.created_at_unix_ms".to_string()
            }
        );
        total_sql.push_str(" AND rl.created_at_unix_ms IS NOT NULL");
        let mut total_values: Vec<SeaValue> =
            vec![time_from_unix_ms.into(), time_to_unix_ms.into()];
        let mut total_idx = 3usize;

        if let Some(uid) = user_id {
            total_sql.push_str(&format!(" AND rl.user_id = ${total_idx}"));
            total_values.push(uid.into());
            total_idx += 1;
        }
        let _ = total_idx;

        let total_row = self
            .db
            .read()
            .query_one(self.db.stmt(&total_sql, total_values))
            .await
            .map_err(|e| e.to_string())?;
        let total_row = total_row.ok_or_else(|| "no total row".to_string())?;

        let total_cost_nano_usd: i64 = if is_sqlite {
            total_row.try_get("", "total_cost").unwrap_or(0)
        } else {
            total_row
                .try_get::<Option<Decimal>>("", "total_cost")
                .ok()
                .flatten()
                .and_then(decimal_to_nano_i64)
                .unwrap_or_else(|| {
                    parse_decimal_query_to_i64(
                        total_row
                            .try_get::<Option<String>>("", "total_cost")
                            .unwrap_or(None),
                    )
                })
        };
        let total_calls: i64 = total_row.try_get("", "total_calls").unwrap_or(0);

        // 4. Today stats
        let mut today_sql = format!(
            r#"SELECT
                 COALESCE(SUM({charge_expr}), 0) AS today_cost,
                 COUNT(*) AS today_calls
               FROM request_logs rl
               WHERE {time_col} >= $1"#,
            time_col = if is_sqlite {
                "rl.created_at_unix_ms".to_string()
            } else {
                "rl.created_at_unix_ms".to_string()
            }
        );
        today_sql.push_str(" AND rl.created_at_unix_ms IS NOT NULL");
        let today_start_unix_ms = chrono::DateTime::parse_from_rfc3339(today_start)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let mut today_values: Vec<SeaValue> = vec![today_start_unix_ms.into()];
        let mut today_idx = 2usize;

        if let Some(uid) = user_id {
            today_sql.push_str(&format!(" AND rl.user_id = ${today_idx}"));
            today_values.push(uid.into());
            today_idx += 1;
        }
        let _ = today_idx;

        let today_row = self
            .db
            .read()
            .query_one(self.db.stmt(&today_sql, today_values))
            .await
            .map_err(|e| e.to_string())?;
        let today_row = today_row.ok_or_else(|| "no today row".to_string())?;

        let today_cost_nano_usd: i64 = if is_sqlite {
            today_row.try_get("", "today_cost").unwrap_or(0)
        } else {
            today_row
                .try_get::<Option<Decimal>>("", "today_cost")
                .ok()
                .flatten()
                .and_then(decimal_to_nano_i64)
                .unwrap_or_else(|| {
                    parse_decimal_query_to_i64(
                        today_row
                            .try_get::<Option<String>>("", "today_cost")
                            .unwrap_or(None),
                    )
                })
        };
        let today_calls: i64 = today_row.try_get("", "today_calls").unwrap_or(0);

        Ok(DashboardAnalyticsRaw {
            model_buckets,
            provider_buckets,
            total_cost_nano_usd,
            total_calls,
            today_cost_nano_usd,
            today_calls,
        })
    }
}

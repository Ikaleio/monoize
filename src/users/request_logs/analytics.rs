use super::*;

impl UserStore {
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

        let model_buckets = self
            .load_analytics_model_buckets(
                user_id,
                time_from_unix_ms,
                time_to_unix_ms,
                bucket_count,
                range_ms,
                is_sqlite,
            )
            .await?;

        let provider_buckets = self
            .load_analytics_provider_buckets(
                user_id,
                time_from_unix_ms,
                time_to_unix_ms,
                bucket_count,
                range_ms,
                is_sqlite,
            )
            .await?;

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

    /// Whether any retained request log row belongs to this user (DH-10).
    pub async fn user_has_request_logs(&self, user_id: &str) -> Result<bool, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT 1 AS present FROM request_logs rl WHERE rl.user_id = $1 LIMIT 1",
                vec![user_id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(row.is_some())
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
    ) -> Result<crate::users::PerformanceTargetRaw, String> {
        if brick_count <= 0 || time_to_unix_ms <= time_from_unix_ms {
            return Err("performance window and brick count must be positive".to_string());
        }
        if let Some(ids) = provider_ids
            && ids.is_empty()
        {
            return Ok(crate::users::PerformanceTargetRaw {
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
                Ok(crate::users::PerformanceHourBucketRow {
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

        let (avg_ttft_ms, avg_tps) = self
            .load_performance_target_averages(
                time_from_unix_ms,
                time_to_unix_ms,
                provider_ids,
                model,
            )
            .await?;

        Ok(crate::users::PerformanceTargetRaw {
            hour_buckets,
            avg_ttft_ms,
            avg_tps,
        })
    }

    async fn load_analytics_model_buckets(
        &self,
        user_id: Option<&str>,
        time_from_unix_ms: i64,
        time_to_unix_ms: i64,
        bucket_count: i64,
        range_ms: i64,
        is_sqlite: bool,
    ) -> Result<Vec<AnalyticsModelBucketRow>, String> {
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
                let input_tokens: i64 = row
                    .try_get::<Option<i64>>("", "input_token_count")
                    .map_err(|e| e.to_string())?
                    .unwrap_or(0);
                let cache_read_tokens: i64 = row
                    .try_get::<Option<i64>>("", "cache_read_token_count")
                    .map_err(|e| e.to_string())?
                    .unwrap_or(0);
                Ok(AnalyticsModelBucketRow {
                    bucket_idx: bucket_idx.clamp(0, bucket_count - 1),
                    model,
                    cost_nano,
                    call_count,
                    token_count,
                    input_tokens,
                    cache_read_tokens,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(model_buckets)
    }

    async fn load_analytics_provider_buckets(
        &self,
        user_id: Option<&str>,
        time_from_unix_ms: i64,
        time_to_unix_ms: i64,
        bucket_count: i64,
        range_ms: i64,
        is_sqlite: bool,
    ) -> Result<Vec<AnalyticsProviderBucketRow>, String> {
        let bucket_expr = analytics_bucket_expr(is_sqlite);

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
        if let Some(uid) = user_id {
            prov_sql.push_str(" AND rl.user_id = $6");
            prov_values.push(uid.into());
        }
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

        Ok(provider_buckets)
    }

    async fn load_performance_target_averages(
        &self,
        time_from_unix_ms: i64,
        time_to_unix_ms: i64,
        provider_ids: Option<&[String]>,
        model: Option<&str>,
    ) -> Result<(Option<f64>, Option<f64>), String> {
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

        Ok((avg_ttft_ms, avg_tps))
    }
}

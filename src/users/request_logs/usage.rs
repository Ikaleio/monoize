use super::*;

impl UserStore {
    pub async fn get_users_today_usage(
        &self,
        today_start: &str,
    ) -> Result<Vec<crate::users::UserTodayUsage>, String> {
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
                Ok(crate::users::UserTodayUsage {
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
    ) -> Result<Vec<crate::users::UserUsageRankingRow>, String> {
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
                Ok(crate::users::UserUsageRankingRow {
                    user_id,
                    username,
                    call_count,
                    cost_nano_usd,
                })
            })
            .collect()
    }

    /// Admin dashboard channel spend (admin-dashboard.spec.md AD-2):
    /// per-channel call count and charge aggregate over `[time_from, time_to)`.
    pub async fn get_channels_window_usage(
        &self,
        time_from: &str,
        time_to: &str,
    ) -> Result<Vec<crate::users::ChannelWindowUsage>, String> {
        let is_sqlite = self.db.is_sqlite();
        let time_from_unix_ms = chrono::DateTime::parse_from_rfc3339(time_from)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let time_to_unix_ms = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        if time_from_unix_ms >= time_to_unix_ms {
            return Err("channel usage time range must be positive".to_string());
        }
        let charge_columns = charge_aggregate_columns(!is_sqlite);
        let sql = format!(
            "SELECT rl.channel_id, {charge_columns}, COUNT(*) AS call_count \
             FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 \
               AND rl.created_at_unix_ms < $2 \
               AND rl.created_at_unix_ms IS NOT NULL \
               AND rl.channel_id IS NOT NULL \
             GROUP BY rl.channel_id"
        );
        let rows = self
            .db
            .read()
            .query_all(
                self.db
                    .stmt(&sql, vec![time_from_unix_ms.into(), time_to_unix_ms.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                let channel_id: String =
                    row.try_get("", "channel_id").map_err(|e| e.to_string())?;
                let window_calls: i64 = row.try_get("", "call_count").map_err(|e| e.to_string())?;
                let window_cost_nano_usd = decode_charge_aggregate(&row, !is_sqlite)?
                    .parse::<i128>()
                    .map_err(|_| {
                        "request log charge is outside the signed i128 domain".to_string()
                    })?;
                Ok(crate::users::ChannelWindowUsage {
                    channel_id,
                    window_calls,
                    window_cost_nano_usd,
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
    pub async fn get_user_live_usage(
        &self,
        user_id: &str,
    ) -> Result<crate::users::UserLiveUsage, String> {
        let now_ms = Utc::now().timestamp_millis();
        let from_ms = now_ms - crate::users::LIVE_USAGE_WINDOW_SECONDS * 1000;
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
        Ok(crate::users::UserLiveUsage {
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

    pub async fn get_providers_live_usage(
        &self,
    ) -> Result<HashMap<String, crate::users::ProviderLiveUsage>, String> {
        let now_ms = Utc::now().timestamp_millis();
        let from_ms = now_ms - crate::users::LIVE_USAGE_WINDOW_SECONDS * 1000;
        let sql = "SELECT rl.provider_id, COUNT(*) AS rpm, \
             CAST(COALESCE(SUM(COALESCE(rl.input_tokens, 0)), 0) AS BIGINT) AS input_tokens, \
             CAST(COALESCE(SUM(COALESCE(rl.output_tokens, 0)), 0) AS BIGINT) AS output_tokens \
             FROM request_logs rl \
             WHERE rl.provider_id IS NOT NULL \
               AND rl.status <> 'pending' \
               AND rl.created_at_unix_ms IS NOT NULL \
               AND rl.created_at_unix_ms >= $1 \
               AND rl.created_at_unix_ms < $2 \
             GROUP BY rl.provider_id";
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(sql, vec![from_ms.into(), now_ms.into()]))
            .await
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                let provider_id: String =
                    row.try_get("", "provider_id").map_err(|e| e.to_string())?;
                let rpm = row.try_get("", "rpm").map_err(|e| e.to_string())?;
                let input_tokens: i64 =
                    row.try_get("", "input_tokens").map_err(|e| e.to_string())?;
                let output_tokens: i64 = row
                    .try_get("", "output_tokens")
                    .map_err(|e| e.to_string())?;
                let tpm = input_tokens
                    .checked_add(output_tokens)
                    .ok_or_else(|| "live usage token aggregate overflow".to_string())?;
                Ok((provider_id, crate::users::ProviderLiveUsage { rpm, tpm }))
            })
            .collect()
    }

    /// Admin dashboard spend totals (admin-dashboard.spec.md AD-2):
    /// call count and charge aggregate over `[time_from, time_to)`.
    pub async fn get_window_usage_totals(
        &self,
        time_from: &str,
        time_to: &str,
    ) -> Result<(i64, i128), String> {
        let is_sqlite = self.db.is_sqlite();
        let time_from_unix_ms = chrono::DateTime::parse_from_rfc3339(time_from)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        let time_to_unix_ms = chrono::DateTime::parse_from_rfc3339(time_to)
            .map_err(|e| e.to_string())?
            .timestamp_millis();
        if time_from_unix_ms >= time_to_unix_ms {
            return Err("usage totals time range must be positive".to_string());
        }
        let sql = format!(
            "{}, COUNT(*) AS call_count FROM request_logs rl \
             WHERE rl.created_at_unix_ms >= $1 \
               AND rl.created_at_unix_ms < $2 \
               AND rl.created_at_unix_ms IS NOT NULL",
            charge_aggregate_select(!is_sqlite)
        );
        let row = self
            .db
            .read()
            .query_one(
                self.db
                    .stmt(&sql, vec![time_from_unix_ms.into(), time_to_unix_ms.into()]),
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no window usage aggregate row".to_string())?;
        let window_calls: i64 = row.try_get("", "call_count").map_err(|e| e.to_string())?;
        let window_cost_nano_usd = decode_charge_aggregate(&row, !is_sqlite)?
            .parse::<i128>()
            .map_err(|_| "request log charge is outside the signed i128 domain".to_string())?;
        Ok((window_calls, window_cost_nano_usd))
    }
}

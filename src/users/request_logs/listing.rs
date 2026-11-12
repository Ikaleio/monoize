use super::*;

impl UserStore {
    pub(super) async fn load_routing_name_maps(
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

    pub(super) async fn enrich_request_log_tried_provider_names(
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

    pub async fn list_request_logs_by_user(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
        filters: RequestLogFilters<'_>,
    ) -> Result<(Vec<RequestLogRow>, i64, String), String> {
        let RequestLogFilters {
            model,
            status,
            api_key_id,
            channel_id,
            username: _,
            search,
            time_from,
            time_to,
        } = filters;
        Self::validate_request_log_model_filter(model)?;
        let is_postgres = self.db.is_postgres();
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let channel_id = normalize_request_log_filter(channel_id);
        let search = normalize_request_log_filter(search);
        let filters = RequestLogFilters {
            model: model.as_deref(),
            status: status.as_deref(),
            api_key_id: api_key_id.as_deref(),
            channel_id: channel_id.as_deref(),
            username: None,
            search: search.as_deref(),
            time_from,
            time_to,
        };
        let txn = self
            .db
            .read()
            .begin_with_config(
                is_postgres.then_some(IsolationLevel::RepeatableRead),
                is_postgres.then_some(AccessMode::ReadOnly),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut aggregates_sql = format!(
            "{} FROM request_logs rl WHERE rl.user_id = $1",
            request_log_list_aggregates_select(is_postgres)
        );
        let mut aggregates_values: Vec<SeaValue> = vec![user_id.into()];
        let mut aggregates_idx = 2usize;
        append_request_log_filters(
            &mut aggregates_sql,
            &mut aggregates_values,
            &mut aggregates_idx,
            is_postgres,
            filters,
        )?;
        let aggregates_row = txn
            .query_one(self.db.stmt(&aggregates_sql, aggregates_values))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no request log aggregate row".to_string())?;
        let total: i64 = aggregates_row
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;
        let total_charge_nano_usd = decode_charge_aggregate(&aggregates_row, is_postgres)?;

        let mut rows_sql = "SELECT rl.* FROM request_logs rl WHERE rl.user_id = $1".to_string();
        let mut rows_values: Vec<SeaValue> = vec![user_id.into()];
        let mut rows_idx = 2usize;
        append_request_log_filters(
            &mut rows_sql,
            &mut rows_values,
            &mut rows_idx,
            is_postgres,
            filters,
        )?;
        let rows_sql = enrich_request_log_page_sql(&rows_sql, is_postgres, rows_idx);
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

    pub async fn list_all_request_logs(
        &self,
        limit: i64,
        offset: i64,
        filters: RequestLogFilters<'_>,
    ) -> Result<(Vec<RequestLogRow>, i64, String), String> {
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
        Self::validate_request_log_model_filter(model)?;
        let is_postgres = self.db.is_postgres();
        let model = normalize_request_log_filter(model);
        let status = normalize_request_log_filter(status);
        let api_key_id = normalize_request_log_filter(api_key_id);
        let channel_id = normalize_request_log_filter(channel_id);
        let username = normalize_request_log_filter(username);
        let search = normalize_request_log_filter(search);
        let filters = RequestLogFilters {
            model: model.as_deref(),
            status: status.as_deref(),
            api_key_id: api_key_id.as_deref(),
            channel_id: channel_id.as_deref(),
            username: username.as_deref(),
            search: search.as_deref(),
            time_from,
            time_to,
        };
        let txn = self
            .db
            .read()
            .begin_with_config(
                is_postgres.then_some(IsolationLevel::RepeatableRead),
                is_postgres.then_some(AccessMode::ReadOnly),
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut aggregates_sql = format!(
            "{} FROM request_logs rl WHERE 1 = 1",
            request_log_list_aggregates_select(is_postgres)
        );
        let mut aggregates_values: Vec<SeaValue> = Vec::new();
        let mut aggregates_idx = 1usize;
        append_request_log_filters(
            &mut aggregates_sql,
            &mut aggregates_values,
            &mut aggregates_idx,
            is_postgres,
            filters,
        )?;
        let aggregates_row = txn
            .query_one(self.db.stmt(&aggregates_sql, aggregates_values))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no request log aggregate row".to_string())?;
        let total: i64 = aggregates_row
            .try_get("", "cnt")
            .map_err(|e| e.to_string())?;
        let total_charge_nano_usd = decode_charge_aggregate(&aggregates_row, is_postgres)?;

        let mut rows_sql = "SELECT rl.* FROM request_logs rl WHERE 1 = 1".to_string();
        let mut rows_values: Vec<SeaValue> = Vec::new();
        let mut rows_idx = 1usize;
        append_request_log_filters(
            &mut rows_sql,
            &mut rows_values,
            &mut rows_idx,
            is_postgres,
            filters,
        )?;
        let rows_sql = enrich_request_log_page_sql(&rows_sql, is_postgres, rows_idx);
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
}

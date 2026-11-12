use super::*;

pub struct SyncRunOutcome<'a> {
    pub status: &'a str,
    pub inserted: i32,
    pub updated: i32,
    pub skipped: i32,
    pub deleted: i32,
    pub error: Option<&'a str>,
    pub detail_json: &'a serde_json::Value,
}

impl ModelPriceStore {
    /// MP-Y13/MP-Y16: set-based upsert of fully merged sync rows in
    /// fixed-size chunks below the portable SQLite bound-parameter limit.
    /// Callers resolve ownership and locks before this write.
    pub async fn bulk_upsert_synced(&self, rows: &[ModelPriceRecord]) -> Result<(), String> {
        let result = self.bulk_upsert_synced_chunks(rows).await;
        // DPT-RC6: chunks commit independently, so a failed call may still
        // have changed rows.
        self.price_cache.invalidate();
        result
    }

    async fn bulk_upsert_synced_chunks(&self, rows: &[ModelPriceRecord]) -> Result<(), String> {
        // 15 bound values per row; 60 rows keeps every statement below the
        // portable 999-parameter SQLite bound.
        const CHUNK_SIZE: usize = 60;
        let write_guard = self.db.write().await;
        for chunk in rows.chunks(CHUNK_SIZE) {
            let mut values: Vec<SeaValue> = Vec::with_capacity(chunk.len() * 15);
            let mut tuples = Vec::with_capacity(chunk.len());
            for record in chunk {
                let start = values.len() + 1;
                let locked_fields_json =
                    serde_json::to_string(&record.locked_fields).map_err(|e| e.to_string())?;
                let billing_expr_json = record
                    .billing_expr
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let raw_json =
                    serde_json::to_string(&record.raw_json).map_err(|e| e.to_string())?;
                values.extend([
                    record.model_id.clone().into(),
                    record.billing_mode.clone().into(),
                    record.input_usd_per_1m.clone().into(),
                    record.output_usd_per_1m.clone().into(),
                    record.cache_read_usd_per_1m.clone().into(),
                    record.cache_write_usd_per_1m.clone().into(),
                    record.cache_write_1h_usd_per_1m.clone().into(),
                    record.reasoning_usd_per_1m.clone().into(),
                    record.per_request_usd.clone().into(),
                    billing_expr_json.into(),
                    record.source.clone().into(),
                    locked_fields_json.into(),
                    raw_json.into(),
                    SeaValue::Int(Some(if record.enabled { 1 } else { 0 })),
                    record.updated_at.to_rfc3339().into(),
                ]);
                let placeholders = (start..start + 15)
                    .map(|index| format!("${index}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                tuples.push(format!("({placeholders})"));
            }
            if tuples.is_empty() {
                continue;
            }
            write_guard
                .execute(self.db.stmt(
                    &format!(
                        "INSERT INTO model_prices ({MODEL_PRICE_COLUMNS}) VALUES {} \
                         ON CONFLICT(model_id) DO UPDATE SET \
                           billing_mode=excluded.billing_mode, \
                           input_usd_per_1m=excluded.input_usd_per_1m, \
                           output_usd_per_1m=excluded.output_usd_per_1m, \
                           cache_read_usd_per_1m=excluded.cache_read_usd_per_1m, \
                           cache_write_usd_per_1m=excluded.cache_write_usd_per_1m, \
                           cache_write_1h_usd_per_1m=excluded.cache_write_1h_usd_per_1m, \
                           reasoning_usd_per_1m=excluded.reasoning_usd_per_1m, \
                           per_request_usd=excluded.per_request_usd, \
                           billing_expr=excluded.billing_expr, \
                           source=excluded.source, \
                           locked_fields=excluded.locked_fields, \
                           raw_json=excluded.raw_json, \
                           enabled=excluded.enabled, \
                           updated_at=excluded.updated_at",
                        tuples.join(", ")
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// MP-Y15: chunked delete of source-owned rows by model id.
    pub async fn delete_by_ids_with_source(
        &self,
        model_ids: &[String],
        source: &str,
    ) -> Result<u64, String> {
        let result = self
            .delete_by_ids_with_source_chunks(model_ids, source)
            .await;
        // DPT-RC6: chunks commit independently.
        self.price_cache.invalidate();
        result
    }

    async fn delete_by_ids_with_source_chunks(
        &self,
        model_ids: &[String],
        source: &str,
    ) -> Result<u64, String> {
        let mut deleted = 0u64;
        let write_guard = self.db.write().await;
        for chunk in model_ids.chunks(100) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = (2..=chunk.len() + 1)
                .map(|index| format!("${index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut values: Vec<SeaValue> = vec![source.into()];
            values.extend(chunk.iter().map(|id| SeaValue::from(id.clone())));
            let result = write_guard
                .execute(self.db.stmt(
                    &format!(
                        "DELETE FROM model_prices WHERE source = $1 \
                         AND model_id IN ({placeholders})"
                    ),
                    values,
                ))
                .await
                .map_err(|e| e.to_string())?;
            deleted += result.rows_affected();
        }
        Ok(deleted)
    }

    /// MP-Y16: inserts one run row with `status = "running"`.
    pub async fn insert_sync_run(&self, source: &str) -> Result<PriceSyncRun, String> {
        let run = PriceSyncRun {
            id: uuid::Uuid::new_v4().to_string(),
            source: source.to_string(),
            status: "running".to_string(),
            started_at: Utc::now(),
            finished_at: None,
            inserted: 0,
            updated: 0,
            skipped: 0,
            deleted: 0,
            error: None,
            detail_json: serde_json::json!({}),
        };
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "INSERT INTO price_sync_runs \
                 (id, source, status, started_at, finished_at, inserted, updated, skipped, \
                  deleted, error, detail_json) \
                 VALUES ($1, $2, $3, $4, NULL, 0, 0, 0, 0, NULL, '{}')",
                vec![
                    run.id.clone().into(),
                    run.source.clone().into(),
                    run.status.clone().into(),
                    run.started_at.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(run)
    }

    /// MP-Y16: finalizes a run row with counts and returns the stored row.
    pub async fn finalize_sync_run(
        &self,
        id: &str,
        outcome: SyncRunOutcome<'_>,
    ) -> Result<PriceSyncRun, String> {
        let SyncRunOutcome {
            status,
            inserted,
            updated,
            skipped,
            deleted,
            error,
            detail_json,
        } = outcome;
        let finished_at = Utc::now();
        let detail = serde_json::to_string(detail_json).map_err(|e| e.to_string())?;
        self.db
            .write()
            .await
            .execute(self.db.stmt(
                "UPDATE price_sync_runs SET status = $2, finished_at = $3, inserted = $4, \
                 updated = $5, skipped = $6, deleted = $7, error = $8, detail_json = $9 \
                 WHERE id = $1",
                vec![
                    id.into(),
                    status.into(),
                    finished_at.to_rfc3339().into(),
                    SeaValue::Int(Some(inserted)),
                    SeaValue::Int(Some(updated)),
                    SeaValue::Int(Some(skipped)),
                    SeaValue::Int(Some(deleted)),
                    error.map(|e| e.to_string()).into(),
                    detail.into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, source, status, started_at, finished_at, inserted, updated, \
                 skipped, deleted, error, detail_json FROM price_sync_runs WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "sync run row missing after finalize".to_string())?;
        row_to_sync_run(&row)
    }

    /// MP-A5: most recent runs first, bounded limit.
    pub async fn list_sync_runs(&self, limit: u64) -> Result<Vec<PriceSyncRun>, String> {
        let limit = limit.clamp(1, 100);
        let rows = self
            .db
            .read()
            .query_all(self.db.stmt(
                &format!(
                    "SELECT id, source, status, started_at, finished_at, inserted, updated, \
                     skipped, deleted, error, detail_json FROM price_sync_runs \
                     ORDER BY started_at DESC LIMIT {limit}"
                ),
                vec![],
            ))
            .await
            .map_err(|e| e.to_string())?;
        rows.iter().map(row_to_sync_run).collect()
    }
}

fn row_to_sync_run(row: &QueryResult) -> Result<PriceSyncRun, String> {
    let started_at_raw: String = row.try_get("", "started_at").map_err(|e| e.to_string())?;
    let finished_at_raw: Option<String> =
        row.try_get("", "finished_at").map_err(|e| e.to_string())?;
    let detail_raw: String = row.try_get("", "detail_json").map_err(|e| e.to_string())?;
    Ok(PriceSyncRun {
        id: row.try_get("", "id").map_err(|e| e.to_string())?,
        source: row.try_get("", "source").map_err(|e| e.to_string())?,
        status: row.try_get("", "status").map_err(|e| e.to_string())?,
        started_at: parse_time(&started_at_raw, "started_at")?,
        finished_at: finished_at_raw
            .map(|raw| parse_time(&raw, "finished_at"))
            .transpose()?,
        inserted: row.try_get("", "inserted").map_err(|e| e.to_string())?,
        updated: row.try_get("", "updated").map_err(|e| e.to_string())?,
        skipped: row.try_get("", "skipped").map_err(|e| e.to_string())?,
        deleted: row.try_get("", "deleted").map_err(|e| e.to_string())?,
        error: row.try_get("", "error").map_err(|e| e.to_string())?,
        detail_json: serde_json::from_str(&detail_raw)
            .map_err(|error| format!("invalid detail_json: {error}"))?,
    })
}

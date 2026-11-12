use super::*;

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
    pub(super) async fn cleanup_expired_request_logs_batched(
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
}

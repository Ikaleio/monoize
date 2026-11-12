use super::*;

impl RequestCaptureStore {
    pub fn new(database_dsn: &str) -> Self {
        Self {
            dump_dir: Arc::new(data_dir_from_database_dsn(database_dsn).join("dumps")),
            limits: RequestCaptureLimits::from_env(),
            db: None,
            runtime: None,
            ready: tokio::sync::broadcast::channel(256).0,
        }
    }

    /// Attach the database pool used for `request_capture_records` metadata
    /// rows (RCD-M3). Without a pool, dumps are written but no metadata row
    /// is recorded, so the capture stays unreachable through the detail API.
    pub fn with_db(mut self, db: DbPool) -> Self {
        self.db = Some(db);
        self
    }

    /// Attach the runtime config used to read the global size budget
    /// (RCD-R6) at cleanup-pass and background-write execution time.
    pub fn with_runtime(mut self, runtime: Arc<RwLock<MonoizeRuntimeConfig>>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn dump_dir(&self) -> &Path {
        &self.dump_dir
    }

    /// Subscribe to persisted capture identifiers. Lagged receivers must reload their snapshot.
    pub fn subscribe_ready(&self) -> tokio::sync::broadcast::Receiver<CaptureReady> {
        self.ready.subscribe()
    }

    /// RCD-C9 pre-check without starting a session. Handlers that must build
    /// a capture-only `raw_input` representation before sessions exist (the
    /// Image API multipart ingress, RCD-D4a) use this to skip that work
    /// entirely when no session could start.
    pub(crate) async fn would_start_session(
        &self,
        runtime: &RwLock<MonoizeRuntimeConfig>,
        auth: &AuthResult,
    ) -> bool {
        let rt = runtime.read().await;
        rt.request_capture_enabled
            && auth.request_capture_mode.should_start_capture()
            && auth.api_key_id.is_some()
            && auth.user_id.is_some()
    }

    pub(crate) async fn maybe_start_session(
        &self,
        runtime: &RwLock<MonoizeRuntimeConfig>,
        auth: &AuthResult,
        request_id: Option<String>,
        downstream_protocol: CaptureDownstreamProtocol,
        is_stream: bool,
    ) -> Option<RequestCaptureSession> {
        let rt = runtime.read().await;
        if !rt.request_capture_enabled || !auth.request_capture_mode.should_start_capture() {
            return None;
        }
        let api_key_id = auth.api_key_id.clone()?;
        let user_id = auth.user_id.clone()?;
        let (request_id, omitted_request_id_bytes) = match request_id {
            Some(request_id) => {
                let retained = truncate_utf8(&request_id, MAX_CAPTURE_IDENTIFIER_BYTES);
                let omitted = request_id.len().saturating_sub(retained.len());
                (Some(retained), omitted)
            }
            None => (None, 0),
        };
        let initial_truncation = CaptureTruncation {
            omitted_bytes: omitted_request_id_bytes,
            ..CaptureTruncation::default()
        };
        Some(RequestCaptureSession {
            store: self.clone(),
            request_id,
            created_at: Utc::now(),
            api_key_id,
            user_id,
            downstream_protocol,
            is_stream,
            mode: auth.request_capture_mode,
            retention: auth.request_capture_retention,
            attempts: Arc::new(Mutex::new(Vec::new())),
            limits: self.limits,
            truncation: Arc::new(Mutex::new(initial_truncation)),
        })
    }

    /// RCD-R2: one pass at startup, then hourly.
    pub fn spawn_cleanup_task(&self) {
        let store = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                store.run_cleanup_pass().await;
            }
        });
    }

    /// RCD-R2 step ordering; RCD-R4: one failed step never stops the next.
    async fn run_cleanup_pass(&self) {
        if let Err(err) = self.cleanup_ttl_expired().await {
            tracing::warn!("request capture TTL cleanup failed: {err}");
        }
        if let Err(err) = self.cleanup_orphan_files().await {
            tracing::warn!("request capture orphan cleanup failed: {err}");
        }
        if let Err(err) = self.enforce_size_budget().await {
            tracing::warn!("request capture size rotation failed: {err}");
        }
    }

    /// RCD-R3: delete every dump whose metadata row has expired, file first,
    /// then the rows, in batches of `CLEANUP_BATCH_SIZE`.
    async fn cleanup_ttl_expired(&self) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        let now_ms = Utc::now().timestamp_millis();
        loop {
            let rows = db
                .read()
                .query_all(db.stmt(
                    &format!(
                        "SELECT file_name FROM request_capture_records \
                         WHERE expires_at_unix_ms <= $1 \
                         ORDER BY expires_at_unix_ms ASC LIMIT {CLEANUP_BATCH_SIZE}"
                    ),
                    vec![now_ms.into()],
                ))
                .await
                .map_err(|err| err.to_string())?;
            let file_names: Vec<String> = rows
                .into_iter()
                .map(|row| row.try_get("", "file_name").map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            if file_names.is_empty() {
                return Ok(());
            }
            let batch_len = file_names.len();
            // RCD-R4: file-deletion failures are logged inside and never stop
            // row deletion; a leftover file becomes an RCD-R5a orphan.
            self.remove_dump_files(file_names.clone()).await;
            self.delete_records_by_file_names(&file_names).await?;
            if batch_len < CLEANUP_BATCH_SIZE {
                return Ok(());
            }
        }
    }

    /// RCD-R5a: delete direct regular files older than the 24-hour horizon
    /// that have no metadata row. Skipped without a database pool because
    /// row existence cannot be evaluated.
    async fn cleanup_orphan_files(&self) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        let dump_dir = self.dump_dir.clone();
        let horizon = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(ORPHAN_FILE_HORIZON_SECS))
            .unwrap_or(std::time::UNIX_EPOCH);
        let candidates =
            tokio::task::spawn_blocking(move || list_files_older_than(&dump_dir, horizon))
                .await
                .map_err(|err| err.to_string())??;
        for chunk in candidates.chunks(CLEANUP_BATCH_SIZE) {
            let placeholders = (0..chunk.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let values: Vec<sea_orm::Value> =
                chunk.iter().map(|name| name.as_str().into()).collect();
            let rows = db
                .read()
                .query_all(db.stmt(
                    &format!(
                        "SELECT file_name FROM request_capture_records \
                         WHERE file_name IN ({placeholders})"
                    ),
                    values,
                ))
                .await
                .map_err(|err| err.to_string())?;
            let registered: std::collections::HashSet<String> = rows
                .into_iter()
                .map(|row| row.try_get("", "file_name").map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let orphans: Vec<String> = chunk
                .iter()
                .filter(|name| !registered.contains(*name))
                .cloned()
                .collect();
            self.remove_dump_files(orphans).await;
        }
        Ok(())
    }

    /// RCD-R6/RCD-R7: while registered dumps exceed the non-zero budget,
    /// delete strictly oldest-first until the total fits.
    pub(super) async fn enforce_size_budget(&self) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        let budget = match self.runtime.as_ref() {
            Some(runtime) => runtime.read().await.request_capture_max_total_bytes,
            None => crate::settings::DEFAULT_REQUEST_CAPTURE_MAX_TOTAL_BYTES,
        };
        if budget == 0 {
            return Ok(());
        }
        let budget = i64::try_from(budget).unwrap_or(i64::MAX);
        loop {
            // Postgres SUM(bigint) yields NUMERIC, so cast back to BIGINT for
            // a uniform i64 read on both backends.
            let total: i64 = db
                .read()
                .query_one(db.stmt(
                    "SELECT CAST(COALESCE(SUM(size_bytes), 0) AS BIGINT) AS total \
                     FROM request_capture_records",
                    vec![],
                ))
                .await
                .map_err(|err| err.to_string())?
                .map(|row| row.try_get("", "total").map_err(|e| e.to_string()))
                .transpose()?
                .unwrap_or(0);
            if total <= budget {
                return Ok(());
            }
            let excess = total - budget;
            let rows = db
                .read()
                .query_all(db.stmt(
                    &format!(
                        "SELECT file_name, size_bytes FROM request_capture_records \
                         ORDER BY created_at_unix_ms ASC, file_name ASC LIMIT {CLEANUP_BATCH_SIZE}"
                    ),
                    vec![],
                ))
                .await
                .map_err(|err| err.to_string())?;
            let mut victims: Vec<String> = Vec::new();
            let mut freed: i64 = 0;
            for row in rows {
                let file_name: String = row.try_get("", "file_name").map_err(|e| e.to_string())?;
                let size_bytes: i64 = row.try_get("", "size_bytes").map_err(|e| e.to_string())?;
                victims.push(file_name);
                freed = freed.saturating_add(size_bytes);
                if freed >= excess {
                    break;
                }
            }
            if victims.is_empty() {
                return Ok(());
            }
            self.remove_dump_files(victims.clone()).await;
            self.delete_records_by_file_names(&victims).await?;
        }
    }

    /// Best-effort file removal: a missing file is not an error (RCD-R3),
    /// other failures are logged and skipped (RCD-R4).
    async fn remove_dump_files(&self, file_names: Vec<String>) {
        if file_names.is_empty() {
            return;
        }
        let dump_dir = self.dump_dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            for file_name in file_names {
                let path = dump_dir.join(&file_name);
                if let Err(err) = std::fs::remove_file(&path)
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!("failed to delete capture dump {file_name}: {err}");
                }
            }
        })
        .await;
        if let Err(err) = result {
            tracing::warn!("capture dump deletion task failed: {err}");
        }
    }

    async fn delete_records_by_file_names(&self, file_names: &[String]) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        for chunk in file_names.chunks(CLEANUP_BATCH_SIZE) {
            let placeholders = (0..chunk.len())
                .map(|index| format!("${}", index + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let values: Vec<sea_orm::Value> =
                chunk.iter().map(|name| name.as_str().into()).collect();
            db.write()
                .await
                .execute(db.stmt(
                    &format!(
                        "DELETE FROM request_capture_records WHERE file_name IN ({placeholders})"
                    ),
                    values,
                ))
                .await
                .map_err(|err| err.to_string())?;
        }
        Ok(())
    }

    /// RCV-A1: candidate records for one request id, newest first. When
    /// `user_id` is supplied only that owner's records are considered.
    pub async fn list_capture_records(
        &self,
        request_id: &str,
        user_id: Option<&str>,
    ) -> Result<Vec<CaptureRecordRow>, String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(Vec::new());
        };
        let mut sql = "SELECT file_name, request_id, user_id, api_key_id, created_at, \
             created_at_unix_ms, size_bytes, expires_at_unix_ms \
             FROM request_capture_records WHERE request_id = $1"
            .to_string();
        let mut values: Vec<sea_orm::Value> = vec![request_id.into()];
        if let Some(user_id) = user_id {
            sql.push_str(" AND user_id = $2");
            values.push(user_id.into());
        }
        sql.push_str(" ORDER BY created_at_unix_ms DESC, file_name DESC");
        let rows = db
            .read()
            .query_all(db.stmt(&sql, values))
            .await
            .map_err(|err| err.to_string())?;
        rows.into_iter()
            .map(|row| {
                Ok(CaptureRecordRow {
                    file_name: row.try_get("", "file_name").map_err(|e| e.to_string())?,
                    request_id: row.try_get("", "request_id").map_err(|e| e.to_string())?,
                    user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                    api_key_id: row.try_get("", "api_key_id").map_err(|e| e.to_string())?,
                    created_at: row.try_get("", "created_at").map_err(|e| e.to_string())?,
                    created_at_unix_ms: row
                        .try_get("", "created_at_unix_ms")
                        .map_err(|e| e.to_string())?,
                    size_bytes: row.try_get("", "size_bytes").map_err(|e| e.to_string())?,
                    expires_at_unix_ms: row
                        .try_get("", "expires_at_unix_ms")
                        .map_err(|e| e.to_string())?,
                })
            })
            .collect()
    }

    /// RCV-A3: on-demand dump read on a blocking-capable executor. Returns
    /// `Ok(None)` when the file no longer exists (RCV-A8 stale record).
    /// RCD-Z6: a zstd magic prefix selects bounded decompression; any other
    /// content is returned as raw bytes, which keeps legacy uncompressed
    /// dumps readable.
    pub async fn read_dump_file(&self, file_name: &str) -> Result<Option<Vec<u8>>, DumpReadError> {
        // Defense in depth: recorded file names never contain separators, but
        // reject any that would escape the dump directory.
        if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
            return Err(DumpReadError::Io(
                "invalid capture dump file name".to_string(),
            ));
        }
        let path = self.dump_dir.join(file_name);
        let max_decompressed_bytes = self.limits.max_session_bytes;
        tokio::task::spawn_blocking(move || {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(err) => return Err(DumpReadError::Io(err.to_string())),
            };
            if bytes.len() >= ZSTD_MAGIC.len() && bytes[..ZSTD_MAGIC.len()] == ZSTD_MAGIC {
                return decompress_bounded(&bytes, max_decompressed_bytes)
                    .map(Some)
                    .map_err(DumpReadError::Unreadable);
            }
            Ok(Some(bytes))
        })
        .await
        .map_err(|err| DumpReadError::Io(err.to_string()))?
    }

    /// RCV-A8: drop a stale metadata row whose dump file no longer exists.
    pub async fn delete_capture_record(&self, file_name: &str) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        db.write()
            .await
            .execute(db.stmt(
                "DELETE FROM request_capture_records WHERE file_name = $1",
                vec![file_name.into()],
            ))
            .await
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    /// RCD-M3: upsert one metadata row immediately after a dump write.
    pub(super) async fn insert_capture_record(
        &self,
        record: &CaptureRecordRow,
    ) -> Result<(), String> {
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        db.write()
            .await
            .execute(db.stmt(
                "INSERT INTO request_capture_records \
                 (file_name, request_id, user_id, api_key_id, created_at, created_at_unix_ms, size_bytes, expires_at_unix_ms) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
                 ON CONFLICT (file_name) DO UPDATE SET \
                 request_id = excluded.request_id, user_id = excluded.user_id, \
                 api_key_id = excluded.api_key_id, created_at = excluded.created_at, \
                 created_at_unix_ms = excluded.created_at_unix_ms, size_bytes = excluded.size_bytes, \
                 expires_at_unix_ms = excluded.expires_at_unix_ms",
                vec![
                    record.file_name.as_str().into(),
                    record.request_id.as_str().into(),
                    record.user_id.as_str().into(),
                    record.api_key_id.as_str().into(),
                    record.created_at.as_str().into(),
                    record.created_at_unix_ms.into(),
                    record.size_bytes.into(),
                    record.expires_at_unix_ms.into(),
                ],
            ))
            .await
            .map_err(|err| err.to_string())?;
        let _ = self.ready.send(CaptureReady {
            request_id: record.request_id.clone(),
            user_id: record.user_id.clone(),
        });
        Ok(())
    }

    /// RCD-Z1/RCD-Z4/RCD-S11: compress, then write via temporary file plus
    /// atomic rename; compression failure falls back to the raw `.json`
    /// shape. Returns the final file name and its on-disk byte length
    /// (RCD-Z8).
    pub(super) async fn write_dump(
        &self,
        request_id: Option<&str>,
        created_at: chrono::DateTime<Utc>,
        bytes: Vec<u8>,
    ) -> Result<(String, i64), String> {
        let dump_dir = self.dump_dir.clone();
        let prefix = request_id_prefix(request_id);
        let timestamp = created_at.format("%Y%m%dT%H%M%S%3fZ").to_string();
        let base_name = format!("{prefix}_{timestamp}");
        tokio::task::spawn_blocking(move || {
            let (extension, payload) = match zstd::encode_all(&bytes[..], ZSTD_COMPRESSION_LEVEL) {
                Ok(compressed) => (".json.zst", compressed),
                Err(err) => {
                    tracing::warn!("zstd compression failed; storing raw capture dump: {err}");
                    (".json", bytes)
                }
            };
            let size_bytes = payload.len() as i64;
            std::fs::create_dir_all(&*dump_dir).map_err(|err| err.to_string())?;
            // Appended (not `with_extension`) so multi-dot names survive; an
            // abandoned tmp file is bounded by RCD-R5a orphan cleanup.
            let tmp_path = dump_dir.join(format!(
                "{base_name}{extension}.tmp.{}",
                uuid::Uuid::new_v4().to_string().replace('-', "")
            ));
            std::fs::write(&tmp_path, payload).map_err(|err| err.to_string())?;
            // RCD-S6a/RCD-S11: publish via hard_link, which atomically fails
            // with AlreadyExists instead of overwriting. This is what keeps
            // same-millisecond RCD-C16 fan-out sessions (same prefix, same
            // timestamp) from silently destroying each other's dump.
            let mut k: u32 = 0;
            let filename = loop {
                let candidate = if k == 0 {
                    format!("{base_name}{extension}")
                } else {
                    format!("{base_name}_{k}{extension}")
                };
                match std::fs::hard_link(&tmp_path, dump_dir.join(&candidate)) {
                    Ok(()) => break candidate,
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => k += 1,
                    Err(_) => {
                        // RCD-S11 fallback for filesystems without hard
                        // links: plain rename, which can overwrite on a
                        // same-name race but never exposes a partial file.
                        std::fs::rename(&tmp_path, dump_dir.join(&candidate))
                            .map_err(|err| err.to_string())?;
                        return Ok((candidate, size_bytes));
                    }
                }
            };
            let _ = std::fs::remove_file(&tmp_path);
            Ok::<(String, i64), String>((filename, size_bytes))
        })
        .await
        .map_err(|err| err.to_string())?
    }
}

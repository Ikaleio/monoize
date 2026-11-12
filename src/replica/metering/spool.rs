use super::*;

// ---------------------------------------------------------------------------
// Durable delta spool (M3)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DeltaSpool {
    dir: PathBuf,
    max_bytes: u64,
    bytes: std::sync::atomic::AtomicU64,
    io_lock: tokio::sync::Mutex<()>,
}

impl DeltaSpool {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Result<Self, String> {
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("metering_spool_unwritable: create dir: {error}"))?;
        let probe = dir.join(format!(".write-probe-{}", uuid::Uuid::new_v4().simple()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
            }
            Err(error) => {
                return Err(format!("metering_spool_unwritable: {error}"));
            }
        }
        let mut total = 0u64;
        let entries =
            std::fs::read_dir(&dir).map_err(|error| format!("open delta spool dir: {error}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            } else if path.file_name().and_then(|name| name.to_str())
                != Some(REPLICA_IDENTITY_FILE_NAME)
            {
                // Remove stale temporary files from an interrupted previous run; the
                // persisted replica identity (M9b) must survive this cleanup.
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(Self {
            dir,
            max_bytes: max_bytes.max(1),
            bytes: std::sync::atomic::AtomicU64::new(total),
            io_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn list_json_files(&self) -> Vec<(String, u64)> {
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        return None;
                    }
                    let size = entry.metadata().ok()?.len();
                    let name = path.file_name()?.to_str()?.to_string();
                    Some((name, size))
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn reconstruct_pending_amounts(&self) -> Vec<(String, i128)> {
        let mut names = self.list_json_files();
        names.sort_by(|left, right| left.0.cmp(&right.0));
        let mut amounts = Vec::new();
        for (name, _) in names {
            let path = self.dir.join(&name);
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<BalanceDelta>(&bytes) {
                    Ok(delta) => {
                        let Some(subject) = delta_subject(&delta) else {
                            continue;
                        };
                        let Ok(amount) = delta.amount_nano_usd.trim().parse::<i128>() else {
                            continue;
                        };
                        amounts.push((subject, amount));
                    }
                    Err(error) => {
                        tracing::warn!(
                            file = %name,
                            error = %error,
                            "skipping unreadable delta spool file during pending reconstruction"
                        );
                    }
                },
                Err(error) => {
                    tracing::warn!(
                        file = %name,
                        error = %error,
                        "skipping unreadable delta spool file during pending reconstruction"
                    );
                }
            }
        }
        amounts
    }

    pub fn pending_files(&self) -> usize {
        self.list_json_files().len()
    }

    /// `(pending file count, total pending bytes)` over the durable spool.
    pub fn pending_stats(&self) -> (usize, u64) {
        let files = self.list_json_files();
        let bytes = files.iter().map(|(_, size)| *size).sum();
        (files.len(), bytes)
    }

    pub fn dir_display(&self) -> String {
        self.dir.display().to_string()
    }

    pub async fn enqueue(&self, delta: &BalanceDelta) -> Result<(), String> {
        let payload = serde_json::to_vec(delta).map_err(|error| error.to_string())?;
        let _io_guard = self.io_lock.lock().await;
        let current = self.bytes.load(Ordering::Acquire);
        if current + payload.len() as u64 > self.max_bytes {
            return Err(format!(
                "replica metering spool quota exhausted ({}/{})",
                current, self.max_bytes
            ));
        }
        let name = format!(
            "{:020}-{}.json",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4().simple()
        );
        let final_path = self.dir.join(&name);
        let tmp_path = self.dir.join(format!(".tmp-{name}"));
        write_durable(&tmp_path, &payload).await?;
        if let Err(error) = tokio::fs::rename(&tmp_path, &final_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(format!("publish delta spool file: {error}"));
        }
        if let Err(error) = sync_dir(&self.dir).await {
            tracing::warn!(error = %error, "delta spool directory sync failed");
        }
        self.bytes.fetch_add(payload.len() as u64, Ordering::AcqRel);
        Ok(())
    }

    pub async fn load_batch(&self, max_entries: usize) -> Vec<(PathBuf, u64, BalanceDelta)> {
        let _io_guard = self.io_lock.lock().await;
        let mut names: Vec<(String, u64)> = self.list_json_files();
        // Oldest first: the timestamp prefix sorts lexicographically.
        names.sort_by(|left, right| left.0.cmp(&right.0));
        let mut loaded = Vec::with_capacity(names.len());
        for (name, size) in names.into_iter().take(max_entries) {
            let path = self.dir.join(&name);
            match tokio::fs::read(&path).await {
                Ok(bytes) => match serde_json::from_slice::<BalanceDelta>(&bytes) {
                    Ok(delta) => loaded.push((path, size, delta)),
                    Err(error) => {
                        tracing::warn!(file = %name, error = %error, "skipping corrupt delta spool file");
                        let _ = tokio::fs::remove_file(&path).await;
                        self.bytes.fetch_sub(size, Ordering::AcqRel);
                    }
                },
                Err(error) => {
                    tracing::warn!(file = %name, error = %error, "reading delta spool file failed");
                }
            }
        }
        loaded
    }

    pub async fn release(&self, files: &[(PathBuf, u64)]) {
        let _io_guard = self.io_lock.lock().await;
        for (path, size) in files {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = %error, "removing shipped delta file failed");
                    continue;
                }
            }
            self.bytes.fetch_sub(*size, Ordering::AcqRel);
        }
    }
}

async fn write_durable(path: &std::path::Path, payload: &[u8]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| format!("create temp file: {error}"))?;
    file.write_all(payload)
        .await
        .map_err(|error| format!("write temp file: {error}"))?;
    file.sync_all()
        .await
        .map_err(|error| format!("sync temp file: {error}"))?;
    Ok(())
}

async fn sync_dir(dir: &std::path::Path) -> Result<(), String> {
    let file = tokio::fs::File::open(dir)
        .await
        .map_err(|error| error.to_string())?;
    file.sync_all().await.map_err(|error| error.to_string())
}

use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DEFAULT_CACHE_TTL_SECONDS: u64 = 60 * 60;
const DEFAULT_CACHE_SWEEP_INTERVAL_SECONDS: u64 = 5 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedImagePayload {
    pub media_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone)]
pub struct ImageTransformCache {
    root: PathBuf,
    ttl: Duration,
}

impl ImageTransformCache {
    pub async fn from_env() -> Result<Self, String> {
        let root = std::env::var("MONOIZE_IMAGE_TRANSFORM_CACHE_DIR")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(default_cache_root);
        Self::new(root, Duration::from_secs(DEFAULT_CACHE_TTL_SECONDS)).await
    }

    pub async fn new(root: PathBuf, ttl: Duration) -> Result<Self, String> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|err| format!("create image transform cache dir {}: {err}", root.display()))?;
        Ok(Self { root, ttl })
    }

    pub fn default_cleanup_interval() -> Duration {
        Duration::from_secs(DEFAULT_CACHE_SWEEP_INTERVAL_SECONDS)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn read_if_fresh(&self, key: &str) -> Result<Option<CachedImagePayload>, String> {
        let path = self.path_for(key);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(format!("read cache metadata {}: {err}", path.display())),
        };
        if self.is_expired(metadata.modified().ok()) {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
        let raw = match tokio::fs::read(&path).await {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(format!("read cache file {}: {err}", path.display())),
        };
        match serde_json::from_slice::<CachedImagePayload>(&raw) {
            Ok(payload) => Ok(Some(payload)),
            Err(err) => {
                tracing::warn!(path = %path.display(), "invalid image transform cache entry: {err}");
                let _ = tokio::fs::remove_file(&path).await;
                Ok(None)
            }
        }
    }

    pub async fn write(&self, key: &str, payload: &CachedImagePayload) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|err| format!("ensure image transform cache dir {}: {err}", self.root.display()))?;
        let path = self.path_for(key);
        let tmp = self.tmp_path_for(key);
        let encoded = serde_json::to_vec(payload)
            .map_err(|err| format!("serialize image transform cache entry {}: {err}", path.display()))?;
        tokio::fs::write(&tmp, encoded)
            .await
            .map_err(|err| format!("write image transform cache temp file {}: {err}", tmp.display()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|err| format!("rename image transform cache file {}: {err}", path.display()))?;
        Ok(())
    }

    pub async fn cleanup_expired(&self) -> Result<u64, String> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|err| format!("ensure image transform cache dir {}: {err}", self.root.display()))?;
        let mut removed = 0_u64;
        let mut entries = tokio::fs::read_dir(&self.root)
            .await
            .map_err(|err| format!("read image transform cache dir {}: {err}", self.root.display()))?;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(err) => {
                    return Err(format!(
                        "iterate image transform cache dir {}: {err}",
                        self.root.display()
                    ));
                }
            };
            let metadata = match entry.metadata().await {
                Ok(metadata) => metadata,
                Err(err) => {
                    tracing::warn!(path = %entry.path().display(), "read image transform cache metadata failed: {err}");
                    continue;
                }
            };
            if metadata.is_file() && self.is_expired(metadata.modified().ok()) {
                match tokio::fs::remove_file(entry.path()).await {
                    Ok(()) => removed = removed.saturating_add(1),
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => tracing::warn!(path = %entry.path().display(), "remove expired image transform cache file failed: {err}"),
                }
            }
        }
        Ok(removed)
    }

    pub fn spawn_cleanup_task(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(err) = self.cleanup_expired().await {
                    tracing::warn!("image transform cache cleanup failed: {err}");
                }
            }
        })
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.json"))
    }

    fn tmp_path_for(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.tmp-{}", std::process::id()))
    }

    fn is_expired(&self, modified: Option<SystemTime>) -> bool {
        modified
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > self.ttl)
    }
}

fn default_cache_root() -> PathBuf {
    std::env::temp_dir().join("monoize").join("image-transform-cache")
}

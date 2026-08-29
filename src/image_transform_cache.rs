use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const DEFAULT_CACHE_TTL_SECONDS: u64 = 60 * 60;
const DEFAULT_CACHE_SWEEP_INTERVAL_SECONDS: u64 = 5 * 60;
const DEFAULT_CACHE_MAX_FILES: usize = 2_048;
const DEFAULT_CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_CACHE_MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
const DEFAULT_MAX_ENCODED_BYTES: usize = 20 * 1024 * 1024;
const DEFAULT_MAX_PIXELS: u64 = 40_000_000;
const DEFAULT_MAX_CONCURRENCY: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedImagePayload {
    pub media_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ImageTransformLimits {
    pub cache_max_files: usize,
    pub cache_max_bytes: u64,
    pub cache_max_entry_bytes: u64,
    pub max_encoded_bytes: usize,
    pub max_pixels: u64,
    pub max_concurrency: usize,
}

impl Default for ImageTransformLimits {
    fn default() -> Self {
        Self {
            cache_max_files: DEFAULT_CACHE_MAX_FILES,
            cache_max_bytes: DEFAULT_CACHE_MAX_BYTES,
            cache_max_entry_bytes: DEFAULT_CACHE_MAX_ENTRY_BYTES,
            max_encoded_bytes: DEFAULT_MAX_ENCODED_BYTES,
            max_pixels: DEFAULT_MAX_PIXELS,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
        }
    }
}

impl ImageTransformLimits {
    fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            cache_max_files: positive_env(
                "MONOIZE_IMAGE_TRANSFORM_CACHE_MAX_FILES",
                defaults.cache_max_files,
            ),
            cache_max_bytes: positive_env(
                "MONOIZE_IMAGE_TRANSFORM_CACHE_MAX_BYTES",
                defaults.cache_max_bytes,
            ),
            cache_max_entry_bytes: positive_env(
                "MONOIZE_IMAGE_TRANSFORM_CACHE_MAX_ENTRY_BYTES",
                defaults.cache_max_entry_bytes,
            ),
            max_encoded_bytes: positive_env(
                "MONOIZE_IMAGE_TRANSFORM_MAX_ENCODED_BYTES",
                defaults.max_encoded_bytes,
            ),
            max_pixels: positive_env("MONOIZE_IMAGE_TRANSFORM_MAX_PIXELS", defaults.max_pixels),
            max_concurrency: positive_env(
                "MONOIZE_IMAGE_TRANSFORM_MAX_CONCURRENCY",
                defaults.max_concurrency,
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageTransformCache {
    root: PathBuf,
    ttl: Duration,
    limits: ImageTransformLimits,
    write_lock: Arc<Mutex<()>>,
    metadata: Arc<Mutex<CacheMetadata>>,
    transform_permits: Arc<Semaphore>,
}

impl ImageTransformCache {
    pub async fn from_env() -> Result<Self, String> {
        let root = std::env::var("MONOIZE_IMAGE_TRANSFORM_CACHE_DIR")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(default_cache_root);
        let ttl = Duration::from_secs(positive_env(
            "MONOIZE_IMAGE_TRANSFORM_CACHE_TTL_SECONDS",
            DEFAULT_CACHE_TTL_SECONDS,
        ));
        Self::new_with_limits(root, ttl, ImageTransformLimits::from_env()).await
    }

    pub async fn new(root: PathBuf, ttl: Duration) -> Result<Self, String> {
        Self::new_with_limits(root, ttl, ImageTransformLimits::default()).await
    }

    pub async fn new_with_limits(
        root: PathBuf,
        ttl: Duration,
        limits: ImageTransformLimits,
    ) -> Result<Self, String> {
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|err| format!("create image transform cache dir {}: {err}", root.display()))?;
        remove_stale_temp_files(&root).await?;
        let max_concurrency = limits.max_concurrency.max(1);
        let limits = ImageTransformLimits {
            cache_max_files: limits.cache_max_files.max(1),
            cache_max_bytes: limits.cache_max_bytes.max(1),
            cache_max_entry_bytes: limits.cache_max_entry_bytes.max(1),
            max_encoded_bytes: limits.max_encoded_bytes.max(1),
            max_pixels: limits.max_pixels.max(1),
            max_concurrency,
        };
        let metadata = load_cache_metadata(&root, limits, ttl).await?;
        Ok(Self {
            root,
            ttl,
            limits,
            write_lock: Arc::new(Mutex::new(())),
            metadata: Arc::new(Mutex::new(metadata)),
            transform_permits: Arc::new(Semaphore::new(max_concurrency)),
        })
    }

    pub fn default_cleanup_interval() -> Duration {
        Duration::from_secs(DEFAULT_CACHE_SWEEP_INTERVAL_SECONDS)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn limits(&self) -> ImageTransformLimits {
        self.limits
    }

    pub async fn acquire_transform_permit(&self) -> Result<OwnedSemaphorePermit, String> {
        self.transform_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "image transform concurrency limiter is closed".to_string())
    }

    pub async fn read_if_fresh(&self, key: &str) -> Result<Option<CachedImagePayload>, String> {
        validate_cache_key(key)?;
        // Point reads serialize with replacement so a stale observation cannot delete a newer
        // file for the same key after an atomic rename (an ABA race).
        let _guard = self.write_lock.lock().await;
        let indexed = { self.metadata.lock().await.entries.get(key).cloned() };
        let Some(indexed) = indexed else {
            return Ok(None);
        };
        let path = self.path_for(key);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                self.remove_metadata_only(key).await;
                return Ok(None);
            }
            Err(err) => return Err(format!("read cache metadata {}: {err}", path.display())),
        };
        if metadata.len() > self.limits.cache_max_entry_bytes
            || metadata.len() != indexed.bytes
            || self.is_expired(metadata.modified().ok())
        {
            self.remove_entry_locked(key).await?;
            return Ok(None);
        }
        let raw = match tokio::fs::read(&path).await {
            Ok(raw) => raw,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                self.remove_metadata_only(key).await;
                return Ok(None);
            }
            Err(err) => return Err(format!("read cache file {}: {err}", path.display())),
        };
        match serde_json::from_slice::<CachedImagePayload>(&raw) {
            Ok(payload) => {
                self.metadata.lock().await.touch(key);
                Ok(Some(payload))
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), "invalid image transform cache entry: {err}");
                self.remove_entry_locked(key).await?;
                Ok(None)
            }
        }
    }

    pub async fn write(&self, key: &str, payload: &CachedImagePayload) -> Result<(), String> {
        validate_cache_key(key)?;
        let encoded = serde_json::to_vec(payload)
            .map_err(|err| format!("serialize image transform cache entry: {err}"))?;
        let encoded_len = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
        if encoded_len > self.limits.cache_max_entry_bytes
            || encoded_len > self.limits.cache_max_bytes
        {
            return Err(format!(
                "image transform cache entry is {encoded_len} bytes and exceeds quota"
            ));
        }

        let _guard = self.write_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await.map_err(|err| {
            format!(
                "ensure image transform cache dir {}: {err}",
                self.root.display()
            )
        })?;
        let path = self.path_for(key);
        self.make_room_for(key, encoded_len).await?;
        let tmp = self.tmp_path_for(key);
        tokio::fs::write(&tmp, encoded).await.map_err(|err| {
            format!(
                "write image transform cache temp file {}: {err}",
                tmp.display()
            )
        })?;
        if let Err(err) = tokio::fs::rename(&tmp, &path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(format!(
                "rename image transform cache file {}: {err}",
                path.display()
            ));
        }
        self.metadata
            .lock()
            .await
            .insert(key.to_string(), encoded_len, SystemTime::now());
        Ok(())
    }

    async fn make_room_for(&self, target_key: &str, incoming_bytes: u64) -> Result<(), String> {
        loop {
            let victim = {
                let metadata = self.metadata.lock().await;
                let existing = metadata
                    .entries
                    .get(target_key)
                    .map(|entry| entry.bytes)
                    .unwrap_or(0);
                let projected_files = metadata
                    .entries
                    .len()
                    .saturating_sub(usize::from(existing > 0))
                    .saturating_add(1);
                let projected_bytes = metadata
                    .total_bytes
                    .saturating_sub(existing)
                    .saturating_add(incoming_bytes);
                if projected_files <= self.limits.cache_max_files
                    && projected_bytes <= self.limits.cache_max_bytes
                {
                    return Ok(());
                }
                metadata.oldest_except(target_key)
            };
            let Some(victim) = victim else {
                return Err("image transform cache quota exhausted".to_string());
            };
            self.remove_entry_locked(&victim).await?;
        }
    }

    pub async fn cleanup_expired(&self) -> Result<u64, String> {
        let _guard = self.write_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await.map_err(|err| {
            format!(
                "ensure image transform cache dir {}: {err}",
                self.root.display()
            )
        })?;
        let candidates = self
            .metadata
            .lock()
            .await
            .entries
            .iter()
            .filter(|(_, entry)| {
                self.is_expired(Some(entry.modified))
                    || entry.bytes > self.limits.cache_max_entry_bytes
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let mut removed = 0_u64;
        for key in candidates {
            match self.remove_entry_locked(&key).await {
                Ok(()) => removed = removed.saturating_add(1),
                Err(error) => {
                    tracing::warn!(
                        key,
                        "remove expired image transform cache file failed: {error}"
                    )
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
        self.root
            .join(format!(".{key}.tmp-{}", uuid::Uuid::new_v4().simple()))
    }

    fn is_expired(&self, modified: Option<SystemTime>) -> bool {
        modified
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > self.ttl)
    }

    async fn remove_entry_locked(&self, key: &str) -> Result<(), String> {
        let path = self.path_for(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => self.remove_metadata_only(key).await,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.remove_metadata_only(key).await
            }
            Err(error) => {
                return Err(format!(
                    "remove image cache entry {}: {error}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    async fn remove_metadata_only(&self, key: &str) {
        self.metadata.lock().await.remove(key);
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    bytes: u64,
    modified: SystemTime,
    sequence: u64,
}

#[derive(Debug, Default)]
struct CacheMetadata {
    entries: HashMap<String, CacheEntry>,
    lru: BTreeSet<(u64, String)>,
    total_bytes: u64,
    next_sequence: u64,
}

impl CacheMetadata {
    fn insert(&mut self, key: String, bytes: u64, modified: SystemTime) {
        self.remove(&key);
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.lru.insert((sequence, key.clone()));
        self.entries.insert(
            key,
            CacheEntry {
                bytes,
                modified,
                sequence,
            },
        );
    }

    fn remove(&mut self, key: &str) {
        let Some(entry) = self.entries.remove(key) else {
            return;
        };
        self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        self.lru.remove(&(entry.sequence, key.to_string()));
    }

    fn touch(&mut self, key: &str) {
        let Some(entry) = self.entries.get_mut(key) else {
            return;
        };
        self.lru.remove(&(entry.sequence, key.to_string()));
        entry.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.lru.insert((entry.sequence, key.to_string()));
    }

    fn oldest_except(&self, excluded: &str) -> Option<String> {
        self.lru
            .iter()
            .find(|(_, key)| key != excluded)
            .map(|(_, key)| key.clone())
    }
}

async fn load_cache_metadata(
    root: &Path,
    limits: ImageTransformLimits,
    ttl: Duration,
) -> Result<CacheMetadata, String> {
    let mut discovered = BTreeMap::new();
    let mut total_bytes = 0_u64;
    let mut entries = tokio::fs::read_dir(root)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(key) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let metadata = entry.metadata().await.map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let expired = modified.elapsed().ok().is_some_and(|age| age > ttl);
        if validate_cache_key(key).is_err()
            || metadata.len() > limits.cache_max_entry_bytes
            || expired
        {
            tokio::fs::remove_file(&path).await.map_err(|error| {
                format!(
                    "remove invalid image cache entry {}: {error}",
                    path.display()
                )
            })?;
            continue;
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        discovered.insert((modified, key.to_string()), (path, metadata.len()));
        while discovered.len() > limits.cache_max_files || total_bytes > limits.cache_max_bytes {
            let Some(((_, _), (path, bytes))) = discovered.pop_first() else {
                break;
            };
            tokio::fs::remove_file(&path).await.map_err(|error| {
                format!(
                    "evict startup image cache entry {}: {error}",
                    path.display()
                )
            })?;
            total_bytes = total_bytes.saturating_sub(bytes);
        }
    }
    let mut metadata = CacheMetadata::default();
    for ((modified, key), (_, bytes)) in discovered {
        metadata.insert(key, bytes, modified);
    }
    Ok(metadata)
}

fn positive_env<T>(name: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + From<u8> + Copy,
{
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<T>().ok())
        .filter(|value| *value > T::from(0))
        .unwrap_or(default)
}

fn default_cache_root() -> PathBuf {
    std::env::temp_dir()
        .join("monoize")
        .join("image-transform-cache")
}

fn validate_cache_key(key: &str) -> Result<(), String> {
    if !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("invalid image transform cache key".to_string())
    }
}

async fn remove_stale_temp_files(root: &Path) -> Result<(), String> {
    let mut entries = tokio::fs::read_dir(root)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        let path = entry.path();
        let is_temp = path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with('.') && value.contains(".tmp-"));
        if !is_temp {
            continue;
        }
        let metadata = entry.metadata().await.map_err(|error| error.to_string())?;
        if metadata.is_file() {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

use super::*;

impl UserStore {
    pub fn is_reserved_internal_username(username: &str) -> bool {
        username
            .trim()
            .to_ascii_lowercase()
            .starts_with(RESERVED_INTERNAL_USER_PREFIX)
    }

    pub async fn new(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<crate::users::InsertRequestLog>>,
    ) -> Result<Self, String> {
        Self::new_with_pending_request_logs(db, log_broadcast, Arc::new(dashmap::DashMap::new()))
            .await
    }

    pub async fn new_with_pending_request_logs(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<crate::users::InsertRequestLog>>,
        pending_request_logs: Arc<dashmap::DashMap<String, crate::users::InsertRequestLog>>,
    ) -> Result<Self, String> {
        Self::new_with_pending_request_logs_and_spool_dir(
            db,
            log_broadcast,
            pending_request_logs,
            None,
        )
        .await
    }

    pub async fn new_with_pending_request_logs_and_spool_dir(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<crate::users::InsertRequestLog>>,
        pending_request_logs: Arc<dashmap::DashMap<String, crate::users::InsertRequestLog>>,
        request_log_spool_dir: Option<std::path::PathBuf>,
    ) -> Result<Self, String> {
        Self::new_for_role(
            db,
            log_broadcast,
            pending_request_logs,
            request_log_spool_dir,
            false,
        )
        .await
    }

    /// `is_replica` skips the startup write passes (PRP11): transform-id canonicalization
    /// and expired-session deletion are primary responsibilities.
    pub async fn new_for_role(
        db: crate::db::DbPool,
        log_broadcast: tokio::sync::broadcast::Sender<Vec<crate::users::InsertRequestLog>>,
        pending_request_logs: Arc<dashmap::DashMap<String, crate::users::InsertRequestLog>>,
        request_log_spool_dir: Option<std::path::PathBuf>,
        is_replica: bool,
    ) -> Result<Self, String> {
        use std::time::Duration;
        let store = Self {
            db,
            last_used_batcher: crate::db_cache::LastUsedBatcher::new(),
            request_log_batcher: crate::db_cache::RequestLogBatcher::new_with_spool_dir(
                128,
                request_log_spool_dir,
                log_broadcast,
                pending_request_logs,
            ),
            api_key_cache: crate::db_cache::ApiKeyCache::new(Duration::from_secs(60)),
            balance_cache: crate::db_cache::BalanceCache::new(Duration::from_secs(30)),
            sub_account_balance_cache: crate::db_cache::BalanceCache::new(Duration::from_secs(30)),
            group_ratio_cache: crate::db_cache::SnapshotCache::new(
                crate::db_cache::REQUEST_PATH_CACHE_TTL,
            ),
            registration_lock: Arc::new(tokio::sync::Mutex::new(())),
            api_key_creation_lock: Arc::new(tokio::sync::Mutex::new(())),
            custom_transforms: crate::custom_transforms::CustomTransformSnapshotHandle::default(),
        };
        if !is_replica {
            store.migrate_transform_rule_ids().await?;
            store.cleanup_expired_sessions().await?;
        }
        Ok(store)
    }

    /// Attaches the shared enabled custom-transform snapshot (CJS-AKV-2).
    pub fn with_custom_transforms(
        mut self,
        handle: crate::custom_transforms::CustomTransformSnapshotHandle,
    ) -> Self {
        self.custom_transforms = handle;
        self
    }

    pub fn spawn_background_tasks(&self) {
        self.spawn_background_tasks_for_role(false);
    }

    /// On a replica the DB flush loops, session cleanup, and log retention loops are
    /// replaced by the shipment pipeline (`primary-replica-deployment.spec.md` PRP12).
    pub fn spawn_background_tasks_for_role(&self, is_replica: bool) {
        if !is_replica {
            self.last_used_batcher
                .clone()
                .spawn_flush_task(self.db.clone(), std::time::Duration::from_secs(30));
            self.request_log_batcher
                .clone()
                .spawn_flush_task(self.db.clone(), std::time::Duration::from_secs(2));
        }
        self.api_key_cache
            .clone()
            .spawn_eviction_task(std::time::Duration::from_secs(30));
        self.balance_cache
            .clone()
            .spawn_eviction_task(std::time::Duration::from_secs(30));
        self.sub_account_balance_cache
            .clone()
            .spawn_eviction_task(std::time::Duration::from_secs(30));
        if is_replica {
            return;
        }
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(session_cleanup_interval()).await;
                if let Err(error) = store.cleanup_expired_sessions().await {
                    tracing::warn!(%error, "failed to cleanup expired sessions");
                }
            }
        });
        let store = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    crate::users::request_logs::REQUEST_LOG_RETENTION_INTERVAL_SECS,
                ))
                .await;
                if let Err(e) = store.cleanup_expired_request_logs().await {
                    tracing::warn!("failed to cleanup expired request logs: {e}");
                }
            }
        });
    }

    /// Replica shipment pipeline access (PRP12/M4).
    pub fn last_used_batcher_clone(&self) -> crate::db_cache::LastUsedBatcher {
        self.last_used_batcher.clone()
    }

    /// Replica shipment pipeline access (PRP12/M4).
    pub fn request_log_batcher_clone(&self) -> crate::db_cache::RequestLogBatcher {
        self.request_log_batcher.clone()
    }

    pub async fn flush_all_batchers(&self) {
        self.last_used_batcher.flush(&self.db).await;
        self.request_log_batcher.flush(&self.db).await;
    }
}

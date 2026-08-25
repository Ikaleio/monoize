//! Replica configuration-epoch poller (E3–E4): one single-row, single-column SELECT
//! per tick; a full settings rebuild runs only when the observed epoch changed.

use std::sync::Arc;
use std::time::Duration;

use crate::custom_transforms::CustomTransformStore;
use crate::db::DbPool;
use crate::monoize_routing::MonoizeRuntimeConfig;
use crate::settings::{SettingsStore, read_config_epoch};

pub async fn apply_config_epoch_tick(
    db: &DbPool,
    settings_store: &SettingsStore,
    runtime: &Arc<tokio::sync::RwLock<MonoizeRuntimeConfig>>,
    custom_transform_store: &CustomTransformStore,
    last_applied: &mut u64,
) {
    match read_config_epoch(db).await {
        Ok(observed) if observed == *last_applied => {}
        Ok(observed) => match settings_store.get_all().await {
            Ok(settings) => {
                let next = crate::app::runtime_config_from_settings(&settings);
                *runtime.write().await = next;
                // CJS-RT-7: the same epoch drives the custom-transform snapshot.
                if let Err(error) = custom_transform_store.reload().await {
                    tracing::warn!(error = %error, "replica custom-transform snapshot reload failed");
                    return;
                }
                *last_applied = observed;
            }
            Err(error) => {
                tracing::warn!(error = %error, "replica config refresh failed to read settings");
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "replica config epoch poll failed");
        }
    }
}

pub(crate) fn spawn_config_epoch_poller(
    db: DbPool,
    settings_store: SettingsStore,
    runtime: Arc<tokio::sync::RwLock<MonoizeRuntimeConfig>>,
    custom_transform_store: CustomTransformStore,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_applied: u64 = 0;
        loop {
            ticker.tick().await;
            apply_config_epoch_tick(
                &db,
                &settings_store,
                &runtime,
                &custom_transform_store,
                &mut last_applied,
            )
            .await;
        }
    })
}

//! Replica configuration-epoch poller (E3–E4): one single-row, single-column SELECT
//! per tick; a full settings rebuild runs only when the observed epoch changed.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::app::AppState;
use crate::settings::read_config_epoch;

async fn apply_config_epoch_tick(state: &AppState, last_applied: &mut u64) {
    match read_config_epoch(&state.db_pool).await {
        Ok(observed) if observed == *last_applied => {}
        Ok(observed) => match state.settings_store.get_all().await {
            Ok(settings) => {
                let next = crate::app::runtime_config_from_settings(&settings);
                {
                    let mut health = state.channel_health.lock().await;
                    let mut runtime = state.monoize_runtime.write().await;
                    if runtime.failure_classification_rules != next.failure_classification_rules {
                        state.routing_config_revision.fetch_add(1, Ordering::AcqRel);
                        health.clear();
                    }
                    *runtime = next;
                }
                // CJS-RT-7: the same epoch drives the custom-transform snapshot.
                if let Err(error) = state.custom_transform_store.reload().await {
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
    state: AppState,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_applied: u64 = 0;
        loop {
            ticker.tick().await;
            apply_config_epoch_tick(&state, &mut last_applied).await;
        }
    })
}

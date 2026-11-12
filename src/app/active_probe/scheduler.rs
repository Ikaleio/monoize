use super::*;
use crate::monoize_routing::{MonoizeChannel, MonoizeProvider};

/// Primary-only background loop that probes unhealthy channels and records each
/// successful probe as a billed request log for the probe system user.
pub(in crate::app) struct ActiveProbeScheduler {
    pub(in crate::app) store: MonoizeRoutingStore,
    pub(in crate::app) http_clients: HttpClients,
    pub(in crate::app) runtime: Arc<tokio::sync::RwLock<MonoizeRuntimeConfig>>,
    pub(in crate::app) health: Arc<Mutex<HashMap<String, ChannelHealthState>>>,
    pub(in crate::app) routing_config_revision: Arc<AtomicU64>,
    pub(in crate::app) user_store: UserStore,
    pub(in crate::app) model_price_store: crate::model_price_store::ModelPriceStore,
    pub(in crate::app) user_id: String,
    pub(in crate::app) shutdown: Arc<AtomicBool>,
    /// Keeps shutdown waiting until the loop has exited.
    pub(in crate::app) _task_registration: RequestLogTaskRegistration,
}

/// Per-tick snapshot shared by every channel probed in that tick.
struct ProbeTick<'a> {
    scheduler: &'a ActiveProbeScheduler,
    now: i64,
    runtime: &'a MonoizeRuntimeConfig,
    pricing: &'a ActiveProbePricingSnapshot,
    routing_config_revision: u64,
}

enum ProbeAttemptOutcome {
    Shutdown,
    Skipped,
    Completed(bool),
}

impl ActiveProbeScheduler {
    pub(in crate::app) fn spawn(self) {
        tokio::spawn(self.run());
    }

    fn shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    async fn run(self) {
        'scheduler: loop {
            if self.shutting_down() {
                break;
            }
            sleep(std::time::Duration::from_secs(1)).await;
            if self.shutting_down() {
                break;
            }
            let routing_config_revision = self.routing_config_revision.load(Ordering::Acquire);
            let providers = match self.store.list_active_probe_candidates().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let now = chrono::Utc::now().timestamp();
            let runtime = self.runtime.read().await.clone();
            let pricing = match build_active_probe_pricing_snapshot(
                &self.model_price_store,
                &providers,
                &runtime,
            )
            .await
            {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    tracing::warn!(error = %err, "active probe pricing snapshot failed");
                    ActiveProbePricingSnapshot::default()
                }
            };
            let tick = ProbeTick {
                scheduler: &self,
                now,
                runtime: &runtime,
                pricing: &pricing,
                routing_config_revision,
            };
            for provider in &providers {
                if self.shutting_down() {
                    break 'scheduler;
                }
                if !provider.circuit_breaker_enabled {
                    continue;
                }
                for channel in &provider.channels {
                    if !tick.probe_channel(provider, channel).await {
                        break 'scheduler;
                    }
                }
            }
        }
    }
}

impl ProbeTick<'_> {
    /// Returns `false` when shutdown was observed and the scheduler must stop.
    async fn probe_channel(&self, provider: &MonoizeProvider, channel: &MonoizeChannel) -> bool {
        if self.scheduler.shutting_down() {
            return false;
        }
        if channel.provider_type == crate::monoize_routing::MonoizeProviderType::Replicate {
            return true;
        }
        let active_enabled = channel
            .active_probe_enabled_override
            .or(provider.active_probe_enabled_override)
            .unwrap_or(self.runtime.active_enabled);
        if !active_enabled {
            return true;
        }
        let probe_interval_seconds = channel
            .active_probe_interval_seconds_override
            .or(provider.active_probe_interval_seconds_override)
            .unwrap_or(self.runtime.active_interval_seconds)
            .max(1);
        let probe_success_threshold = channel
            .active_probe_success_threshold_override
            .or(provider.active_probe_success_threshold_override)
            .unwrap_or(self.runtime.active_success_threshold)
            .max(1);
        let Some(model_name) = self
            .select_model(provider, channel, probe_interval_seconds)
            .await
        else {
            return true;
        };
        if !channel.models.contains_key(&model_name) {
            return true;
        }

        let probe_http = match self
            .scheduler
            .http_clients
            .for_channel_proxy(channel.proxy_url.as_deref())
        {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(
                    channel_id = %channel.id,
                    channel_name = %channel.name,
                    provider = %provider.name,
                    error = %error,
                    "active probe skipped because channel proxy could not be built"
                );
                return true;
            }
        };

        let upstream_model = channel
            .models
            .get(&model_name)
            .and_then(|entry| entry.redirect.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&model_name)
            .to_string();
        if self.scheduler.shutting_down() {
            return false;
        }
        let ok = match self
            .execute_probe(provider, channel, &model_name, &upstream_model, &probe_http)
            .await
        {
            ProbeAttemptOutcome::Shutdown => return false,
            ProbeAttemptOutcome::Skipped => return true,
            ProbeAttemptOutcome::Completed(ok) => ok,
        };
        tracing::debug!(
            channel_id = %channel.id,
            channel_name = %channel.name,
            provider = %provider.name,
            probe_model = ?Some(&model_name),
            probe_interval_seconds,
            probe_success_threshold,
            success = ok,
            "active health probe result"
        );
        self.update_health(provider, channel, &model_name, ok, probe_success_threshold)
            .await;
        true
    }

    /// Per-model breakers probe the configured model when it is due, else the first
    /// due model; channel breakers probe the configured or first model when the
    /// channel itself is due.
    async fn select_model(
        &self,
        provider: &MonoizeProvider,
        channel: &MonoizeChannel,
        probe_interval_seconds: u64,
    ) -> Option<String> {
        let now = self.now;
        let configured_model = channel
            .active_probe_model_override
            .clone()
            .or(provider.active_probe_model_override.clone())
            .or(self.runtime.active_probe_model.clone());
        let is_due = |state: &ChannelHealthState| {
            !state.healthy
                && state.cooldown_until.is_none_or(|until| now >= until)
                && state.last_probe_at.is_none_or(|last_probe_at| {
                    now.saturating_sub(last_probe_at) >= probe_interval_seconds as i64
                })
        };
        let guard = self.scheduler.health.lock().await;
        if provider.per_model_circuit_break {
            let mut due_models: Vec<String> = channel
                .models
                .keys()
                .filter(|model| {
                    guard
                        .get(&health_key(&channel.id, Some(model)))
                        .is_some_and(is_due)
                })
                .cloned()
                .collect();
            due_models.sort();
            configured_model
                .filter(|model| due_models.contains(model))
                .or_else(|| due_models.into_iter().next())
        } else {
            let probe_due = guard
                .get(&health_key(&channel.id, None))
                .is_some_and(is_due);
            probe_due
                .then(|| configured_model.or_else(|| channel.models.keys().min().cloned()))
                .flatten()
        }
    }

    /// Reserves and arms the terminal request log, runs the probe, then cancels the
    /// reservation on failure or settles it on success.
    async fn execute_probe(
        &self,
        provider: &MonoizeProvider,
        channel: &MonoizeChannel,
        model_name: &str,
        upstream_model: &str,
        probe_http: &reqwest::Client,
    ) -> ProbeAttemptOutcome {
        let user_store = &self.scheduler.user_store;
        let request_log_reservation = match user_store.reserve_terminal_request_log() {
            Ok(reservation) => reservation,
            Err(error) => {
                tracing::warn!(
                    channel_id = %channel.id,
                    channel_name = %channel.name,
                    provider = %provider.name,
                    probe_model = %model_name,
                    "active probe skipped because request-log spool admission failed: {error}"
                );
                return ProbeAttemptOutcome::Skipped;
            }
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let log_context = ActiveProbeLogContext {
            request_id: &request_id,
            user_id: &self.scheduler.user_id,
            provider_id: &provider.id,
            provider_name: &provider.name,
            provider_type: channel.provider_type,
            channel_multiplier: channel.models.get(model_name).map(|entry| entry.multiplier),
            channel_id: &channel.id,
            channel_name: &channel.name,
            logical_model: model_name,
            upstream_model,
            created_at: chrono::Utc::now(),
        };
        if let Err(error) = user_store
            .arm_terminal_request_log(
                build_active_probe_interrupted_log(log_context),
                &request_log_reservation,
            )
            .await
        {
            tracing::warn!(
                channel_id = %channel.id,
                channel_name = %channel.name,
                provider = %provider.name,
                probe_model = %model_name,
                "active probe skipped because request-log fallback could not be armed: {error}"
            );
            return ProbeAttemptOutcome::Skipped;
        }
        if self.scheduler.shutting_down() {
            if let Err(error) = user_store
                .cancel_terminal_request_log(&request_log_reservation)
                .await
            {
                tracing::error!(
                    channel_id = %channel.id,
                    channel_name = %channel.name,
                    provider = %provider.name,
                    probe_model = %model_name,
                    "active probe request-log reservation cancellation failed: {error}"
                );
            }
            return ProbeAttemptOutcome::Shutdown;
        }
        let started_at = std::time::Instant::now();
        let outcome = probe_channel_completion(
            probe_http,
            channel,
            self.runtime.request_timeout_ms,
            upstream_model,
            channel.provider_type,
            &provider.api_type_overrides,
            false,
        )
        .await;
        if !outcome.ok {
            if let Err(error) = user_store
                .cancel_terminal_request_log(&request_log_reservation)
                .await
            {
                tracing::error!(
                    channel_id = %channel.id,
                    channel_name = %channel.name,
                    provider = %provider.name,
                    probe_model = %model_name,
                    "failed active probe request-log reservation cancellation failed: {error}"
                );
                return ProbeAttemptOutcome::Skipped;
            }
            return ProbeAttemptOutcome::Completed(false);
        }
        if let Err(error) = persist_active_probe_request_log(
            user_store,
            log_context,
            self.pricing.resolve(upstream_model, model_name),
            outcome.usage,
            started_at.elapsed().as_millis() as u64,
            request_log_reservation,
        )
        .await
        {
            tracing::error!(
                channel_id = %channel.id,
                channel_name = %channel.name,
                provider = %provider.name,
                probe_model = %model_name,
                "active probe request log could not be durably enqueued: {error}"
            );
            return ProbeAttemptOutcome::Skipped;
        }
        ProbeAttemptOutcome::Completed(true)
    }

    /// Results computed against a routing config that changed mid-tick are dropped.
    async fn update_health(
        &self,
        provider: &MonoizeProvider,
        channel: &MonoizeChannel,
        model_name: &str,
        ok: bool,
        probe_success_threshold: u32,
    ) {
        let now = self.now;
        let mut guard = self.scheduler.health.lock().await;
        if self
            .scheduler
            .routing_config_revision
            .load(Ordering::Acquire)
            != self.routing_config_revision
        {
            return;
        }
        let state = if provider.per_model_circuit_break {
            match guard.get_mut(&health_key(&channel.id, Some(model_name))) {
                Some(state) => state,
                None => return,
            }
        } else {
            let key = health_key(&channel.id, None);
            if !crate::monoize_routing::prepare_channel_health_insert(&mut guard, &key) {
                return;
            }
            guard.entry(key).or_insert_with(ChannelHealthState::new)
        };
        if ok {
            state.last_probe_at = Some(now);
            state.probe_success_count = state.probe_success_count.saturating_add(1);
            if state.probe_success_count >= probe_success_threshold {
                clear_channel_health_state(state, now);
            }
        } else {
            let cooldown_seconds = channel
                .passive_cooldown_seconds_override
                .unwrap_or(self.runtime.passive_cooldown_seconds)
                .max(1);
            state.healthy = false;
            state.probe_success_count = 0;
            state.last_probe_at = Some(now);
            state.cooldown_until = Some(now + cooldown_seconds as i64);
        }
    }
}

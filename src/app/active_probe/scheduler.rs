use super::*;

pub(in crate::app) struct ActiveProbeScheduler {
    pub(in crate::app) probe_runtime: Arc<tokio::sync::RwLock<MonoizeRuntimeConfig>>,
    pub(in crate::app) probe_health: Arc<Mutex<HashMap<String, ChannelHealthState>>>,
    pub(in crate::app) probe_routing_config_revision: Arc<AtomicU64>,
    pub(in crate::app) probe_user_store: UserStore,
    pub(in crate::app) probe_model_price_store: crate::model_price_store::ModelPriceStore,
    pub(in crate::app) probe_user_id: Option<String>,
    pub(in crate::app) probe_shutdown: Arc<AtomicBool>,
    pub(in crate::app) probe_task_registration: RequestLogTaskRegistration,
    pub(in crate::app) probe_store: MonoizeRoutingStore,
    pub(in crate::app) probe_http_clients: HttpClients,
}

impl ActiveProbeScheduler {
    pub(in crate::app) fn spawn(self) {
        tokio::spawn(self.run());
    }

    async fn run(self) {
        let _probe_task_registration = &self.probe_task_registration;
        let Some(ref probe_user_id) = self.probe_user_id else {
            return;
        };
        'scheduler: loop {
            if self.probe_shutdown.load(Ordering::Acquire) {
                break;
            }
            sleep(std::time::Duration::from_secs(1)).await;
            if self.probe_shutdown.load(Ordering::Acquire) {
                break;
            }
            let routing_config_revision =
                self.probe_routing_config_revision.load(Ordering::Acquire);
            let providers = match self.probe_store.list_active_probe_candidates().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let now = chrono::Utc::now().timestamp();
            let rt_snap = self.probe_runtime.read().await.clone();
            let pricing_snapshot = match build_active_probe_pricing_snapshot(
                &self.probe_model_price_store,
                &providers,
                &rt_snap,
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
                user_id: probe_user_id,
                now,
                runtime: &rt_snap,
                pricing: &pricing_snapshot,
                routing_config_revision,
            };
            for provider in providers {
                if self.probe_shutdown.load(Ordering::Acquire) {
                    break 'scheduler;
                }
                if !provider.circuit_breaker_enabled {
                    continue;
                }
                for channel in &provider.channels {
                    if !tick.probe_channel(&provider, channel).await {
                        break 'scheduler;
                    }
                }
            }
        }
    }
}

struct ProbeTick<'a> {
    scheduler: &'a ActiveProbeScheduler,
    user_id: &'a str,
    now: i64,
    runtime: &'a MonoizeRuntimeConfig,
    pricing: &'a ActiveProbePricingSnapshot,
    routing_config_revision: u64,
}
impl ProbeTick<'_> {
    async fn probe_channel(
        &self,
        provider: &crate::monoize_routing::MonoizeProvider,
        channel: &crate::monoize_routing::MonoizeChannel,
    ) -> bool {
        let scheduler = self.scheduler;
        let probe_shutdown = &scheduler.probe_shutdown;
        let probe_http_clients = &scheduler.probe_http_clients;
        let rt_snap = self.runtime;
        if probe_shutdown.load(Ordering::Acquire) {
            return false;
        }
        if channel.provider_type == crate::monoize_routing::MonoizeProviderType::Replicate {
            return true;
        }
        let active_enabled = channel
            .active_probe_enabled_override
            .or(provider.active_probe_enabled_override)
            .unwrap_or(rt_snap.active_enabled);
        if !active_enabled {
            return true;
        }
        let probe_interval_seconds = channel
            .active_probe_interval_seconds_override
            .or(provider.active_probe_interval_seconds_override)
            .unwrap_or(rt_snap.active_interval_seconds)
            .max(1);
        let probe_success_threshold = channel
            .active_probe_success_threshold_override
            .or(provider.active_probe_success_threshold_override)
            .unwrap_or(rt_snap.active_success_threshold)
            .max(1);
        let probe_model = self
            .select_model(provider, channel, probe_interval_seconds)
            .await;
        let Some(ref model_name) = probe_model else {
            return true;
        };
        if !channel.models.contains_key(model_name) {
            return true;
        }

        let probe_http = match probe_http_clients.for_channel_proxy(channel.proxy_url.as_deref()) {
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
            .get(model_name)
            .and_then(|entry| entry.redirect.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(model_name)
            .to_string();
        if probe_shutdown.load(Ordering::Acquire) {
            return false;
        }
        let ok = match self
            .execute_probe(provider, channel, model_name, &upstream_model, &probe_http)
            .await
        {
            ProbeAttemptOutcome::Shutdown => return false,
            ProbeAttemptOutcome::Skipped => return true,
            ProbeAttemptOutcome::Completed(ok) => ok,
        };
        self.update_health(
            provider,
            channel,
            model_name,
            ok,
            probe_interval_seconds,
            probe_success_threshold,
        )
        .await;
        true
    }
    async fn select_model(
        &self,
        provider: &crate::monoize_routing::MonoizeProvider,
        channel: &crate::monoize_routing::MonoizeChannel,
        probe_interval_seconds: u64,
    ) -> Option<String> {
        let rt_snap = self.runtime;
        let probe_health = &self.scheduler.probe_health;
        let now = self.now;
        let configured_model = channel
            .active_probe_model_override
            .clone()
            .or(provider.active_probe_model_override.clone())
            .or(rt_snap.active_probe_model.clone());
        let first_model = channel.models.keys().min().cloned();
        {
            let guard = probe_health.lock().await;
            if provider.per_model_circuit_break {
                let mut due_models: Vec<String> = channel
                    .models
                    .keys()
                    .filter(|model| {
                        guard
                            .get(&health_key(&channel.id, Some(model)))
                            .is_some_and(|state| {
                                !state.healthy
                                    && state.cooldown_until.is_none_or(|until| now >= until)
                                    && state.last_probe_at.is_none_or(|last_probe_at| {
                                        now.saturating_sub(last_probe_at)
                                            >= probe_interval_seconds as i64
                                    })
                            })
                    })
                    .cloned()
                    .collect();
                due_models.sort();
                configured_model
                    .as_ref()
                    .filter(|model| due_models.contains(model))
                    .cloned()
                    .or_else(|| due_models.into_iter().next())
            } else {
                let state = guard
                    .get(&health_key(&channel.id, None))
                    .cloned()
                    .unwrap_or_else(ChannelHealthState::new);
                let probe_due = !state.healthy
                    && state.cooldown_until.is_none_or(|until| now >= until)
                    && state.last_probe_at.is_none_or(|last_probe_at| {
                        now.saturating_sub(last_probe_at) >= probe_interval_seconds as i64
                    });
                probe_due
                    .then(|| configured_model.clone().or(first_model.clone()))
                    .flatten()
            }
        }
    }
    async fn update_health(
        &self,
        provider: &crate::monoize_routing::MonoizeProvider,
        channel: &crate::monoize_routing::MonoizeChannel,
        model_name: &str,
        ok: bool,
        probe_interval_seconds: u64,
        probe_success_threshold: u32,
    ) {
        let probe_model = Some(model_name);
        let probe_health = &self.scheduler.probe_health;
        let probe_routing_config_revision = &self.scheduler.probe_routing_config_revision;
        let routing_config_revision = self.routing_config_revision;
        let now = self.now;
        let rt_snap = self.runtime;
        tracing::debug!(
            channel_id = %channel.id,
            channel_name = %channel.name,
            provider = %provider.name,
            probe_model = ?probe_model,
            probe_interval_seconds,
            probe_success_threshold,
            success = ok,
            "active health probe result"
        );

        let mut guard = probe_health.lock().await;
        if probe_routing_config_revision.load(Ordering::Acquire) != routing_config_revision {
            return;
        }
        if ok {
            if provider.per_model_circuit_break {
                let key = health_key(&channel.id, Some(model_name));
                if let Some(state) = guard.get_mut(&key) {
                    state.last_probe_at = Some(now);
                    state.probe_success_count = state.probe_success_count.saturating_add(1);
                    if state.probe_success_count >= probe_success_threshold {
                        clear_channel_health_state(state, now);
                    }
                }
            } else {
                let key = health_key(&channel.id, None);
                if !crate::monoize_routing::prepare_channel_health_insert(&mut guard, &key) {
                    return;
                }
                let state = guard.entry(key).or_insert_with(ChannelHealthState::new);
                state.last_probe_at = Some(now);
                state.probe_success_count = state.probe_success_count.saturating_add(1);
                if state.probe_success_count >= probe_success_threshold {
                    clear_channel_health_state(state, now);
                }
            }
        } else {
            let cooldown_seconds = channel
                .passive_cooldown_seconds_override
                .unwrap_or(rt_snap.passive_cooldown_seconds)
                .max(1);
            if provider.per_model_circuit_break {
                let key = health_key(&channel.id, Some(model_name));
                if let Some(state) = guard.get_mut(&key) {
                    state.healthy = false;
                    state.probe_success_count = 0;
                    state.last_probe_at = Some(now);
                    state.cooldown_until = Some(now + cooldown_seconds as i64);
                }
            } else {
                let key = health_key(&channel.id, None);
                if !crate::monoize_routing::prepare_channel_health_insert(&mut guard, &key) {
                    return;
                }
                let state = guard.entry(key).or_insert_with(ChannelHealthState::new);
                state.healthy = false;
                state.probe_success_count = 0;
                state.last_probe_at = Some(now);
                state.cooldown_until = Some(now + cooldown_seconds as i64);
            }
        }
    }
}

enum ProbeAttemptOutcome {
    Shutdown,
    Skipped,
    Completed(bool),
}

impl ProbeTick<'_> {
    async fn execute_probe(
        &self,
        provider: &crate::monoize_routing::MonoizeProvider,
        channel: &crate::monoize_routing::MonoizeChannel,
        model_name: &str,
        upstream_model: &str,
        probe_http: &reqwest::Client,
    ) -> ProbeAttemptOutcome {
        let probe_shutdown = &self.scheduler.probe_shutdown;
        let probe_user_store = &self.scheduler.probe_user_store;
        let probe_user_id = self.user_id;
        let rt_snap = self.runtime;
        let pricing_snapshot = self.pricing;
        let request_log_reservation = match probe_user_store.reserve_terminal_request_log() {
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
        let probe_request_id = uuid::Uuid::new_v4().to_string();
        let probe_created_at = chrono::Utc::now();
        let log_context = ActiveProbeLogContext {
            request_id: &probe_request_id,
            user_id: probe_user_id,
            provider_id: &provider.id,
            provider_name: &provider.name,
            provider_type: channel.provider_type,
            channel_multiplier: channel.models.get(model_name).map(|entry| entry.multiplier),
            channel_id: &channel.id,
            channel_name: &channel.name,
            logical_model: model_name,
            upstream_model,
            created_at: probe_created_at,
        };
        let probe_fallback = build_active_probe_interrupted_log(log_context);
        if let Err(error) = probe_user_store
            .arm_terminal_request_log(probe_fallback, &request_log_reservation)
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
        if probe_shutdown.load(Ordering::Acquire) {
            if let Err(error) = probe_user_store
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
        let probe_started_at = std::time::Instant::now();
        let probe_outcome = probe_channel_completion(
            probe_http,
            channel,
            rt_snap.request_timeout_ms,
            upstream_model,
            channel.provider_type,
            &provider.api_type_overrides,
            false,
        )
        .await;
        let ok = probe_outcome.ok;
        let usage_snapshot = probe_outcome.usage;
        if !ok
            && let Err(error) = probe_user_store
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
        if ok
            && let Err(error) = persist_active_probe_request_log(
                probe_user_store,
                log_context,
                pricing_snapshot.resolve(upstream_model, model_name),
                usage_snapshot,
                probe_started_at.elapsed().as_millis() as u64,
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
        ProbeAttemptOutcome::Completed(ok)
    }
}

use super::*;
use crate::handlers::channel_probe::{
    ChannelProbeOutcome, ProbeRequestSettings, prepare_channel_probe,
};
use crate::monoize_routing::{
    ChannelProbe, MonoizeChannel, MonoizeProvider, MonoizeProviderType, RecoveryMode,
    recovery_deadline, recovery_wait, select_channel_probe_model,
};
use crate::settings::RetryableFailureClass;
use futures_util::StreamExt;

/// Primary-only background loop that probes unhealthy channels and records each
/// successful probe as a billed request log for the probe system user.
pub(in crate::app) struct ActiveProbeScheduler {
    pub(in crate::app) state: AppState,
    pub(in crate::app) user_id: String,
    /// Keeps shutdown waiting until the loop has exited.
    pub(in crate::app) _task_registration: RequestLogTaskRegistration,
}

struct DueKey {
    key: String,
    trip_id: u64,
}

struct ProbeJob<'a> {
    due: &'a DueKey,
    provider: &'a MonoizeProvider,
    channel: &'a MonoizeChannel,
    probe: ChannelProbe<'a>,
    interval: u64,
    success_threshold: u32,
    timeout_ms: u64,
    stream: bool,
    rate_limit_wait: Option<u64>,
}

struct ProbeTick<'a> {
    scheduler: &'a ActiveProbeScheduler,
    pricing: &'a ActiveProbePricingSnapshot,
    settings: &'a ProbeRequestSettings,
    revision: u64,
}

impl ActiveProbeScheduler {
    pub(in crate::app) fn spawn(self) {
        tokio::spawn(self.run());
    }

    fn shutting_down(&self) -> bool {
        self.state.background_shutdown.load(Ordering::Acquire)
    }

    async fn run(self) {
        while !self.shutting_down() {
            sleep(std::time::Duration::from_secs(1)).await;
            if self.shutting_down() {
                break;
            }
            self.tick().await;
        }
    }

    async fn due_keys(&self) -> (u64, HashMap<String, Vec<DueKey>>) {
        let health = self.state.channel_health.lock().await;
        let now = chrono::Utc::now().timestamp();
        let revision = self.state.routing_config_revision.load(Ordering::Acquire);
        let mut keys: HashMap<String, Vec<DueKey>> = HashMap::new();
        for (key, entry) in health.iter() {
            if !entry.is_unhealthy(now)
                || entry.recovery_mode != Some(RecoveryMode::Probe)
                || !entry
                    .next_recovery_at
                    .is_some_and(|deadline| now >= deadline)
            {
                continue;
            }
            if let Some(trip_id) = entry.trip_id {
                let channel_id = key.split_once("::").map_or(key.as_str(), |(id, _)| id);
                keys.entry(channel_id.to_owned()).or_default().push(DueKey {
                    key: key.clone(),
                    trip_id,
                });
            }
        }
        (revision, keys)
    }

    async fn tick(&self) {
        let (revision, due) = self.due_keys().await;
        if due.is_empty() || self.shutting_down() {
            return;
        }
        let providers = match self
            .state
            .monoize_store
            .list_active_probe_candidates()
            .await
        {
            Ok(providers) => providers,
            Err(_) => return,
        };
        let (jobs, reasoning_suffix_map, settings) = {
            let runtime = self.state.monoize_runtime.read().await;
            let jobs = build_probe_jobs(&due, &providers, &runtime);
            if jobs.is_empty() {
                return;
            }
            (
                jobs,
                runtime.reasoning_suffix_map.clone(),
                ProbeRequestSettings::from(&*runtime),
            )
        };
        let pricing = match build_active_probe_pricing_snapshot(
            &self.state.model_price_store,
            jobs.iter().map(|job| job.probe),
            reasoning_suffix_map,
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
            scheduler: self,
            pricing: &pricing,
            settings: &settings,
            revision,
        };
        let mut active = futures_util::stream::FuturesUnordered::new();
        for job in jobs {
            if active.len() == 8 {
                let _ = active.next().await;
            }
            if self.shutting_down() {
                break;
            }
            active.push(tick.run_job(job));
        }
        while active.next().await.is_some() {}
    }
}

fn build_probe_jobs<'a>(
    due: &'a HashMap<String, Vec<DueKey>>,
    providers: &'a [MonoizeProvider],
    runtime: &MonoizeRuntimeConfig,
) -> Vec<ProbeJob<'a>> {
    let mut jobs = Vec::new();
    for provider in providers {
        for channel in &provider.channels {
            if channel.provider_type == MonoizeProviderType::Replicate {
                continue;
            }
            let Some(keys) = due.get(&channel.id) else {
                continue;
            };
            for due in keys {
                let model_key = due.key.split_once("::").map(|(_, model)| model);
                let logical_model = match (provider.per_model_circuit_break, model_key) {
                    (true, Some(model)) => Some(model),
                    (false, None) => select_channel_probe_model(
                        provider,
                        channel,
                        runtime.active_probe_model.as_deref(),
                    ),
                    _ => None,
                };
                let Some(probe) = logical_model
                    .and_then(|model| ChannelProbe::for_model(provider, channel, model))
                else {
                    continue;
                };
                if probe.provider_type == MonoizeProviderType::Replicate {
                    continue;
                }
                jobs.push(ProbeJob {
                    due,
                    provider,
                    channel,
                    probe,
                    interval: runtime.probe_interval(provider, channel),
                    timeout_ms: runtime.request_timeout(provider),
                    stream: probe.is_completion() && runtime.probe_stream(provider, channel),
                    success_threshold: channel
                        .active_probe_success_threshold_override
                        .or(provider.active_probe_success_threshold_override)
                        .unwrap_or(runtime.active_success_threshold)
                        .max(1),
                    rate_limit_wait: runtime.rate_limit_wait(provider, channel),
                });
            }
        }
    }
    jobs
}

fn log_probe_result(
    provider: &MonoizeProvider,
    channel: &MonoizeChannel,
    model: &str,
    success: bool,
) {
    tracing::debug!(channel_id = %channel.id, provider = %provider.name, probe_model = %model, success, "active health probe result");
}

impl ProbeTick<'_> {
    async fn run_job(&self, job: ProbeJob<'_>) {
        if self.scheduler.shutting_down() {
            return;
        }
        let http = match self
            .scheduler
            .state
            .http_clients
            .for_channel_proxy(job.channel.proxy_url.as_deref())
        {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(channel_id = %job.channel.id, %error, "active probe skipped because channel proxy could not be built");
                return;
            }
        };
        let Some(outcome) = self.execute_probe(&job, &http).await else {
            return;
        };
        self.update_health(&job, &outcome).await;
    }

    fn current_trip(entry: &ChannelHealthState, due: &DueKey, now: i64) -> bool {
        entry.trip_id == Some(due.trip_id) && entry.is_unhealthy(now)
    }

    async fn can_dispatch(&self, due: &DueKey) -> bool {
        let health = self.scheduler.state.channel_health.lock().await;
        self.scheduler
            .state
            .routing_config_revision
            .load(Ordering::Acquire)
            == self.revision
            && health
                .get(&due.key)
                .is_some_and(|entry| Self::current_trip(entry, due, chrono::Utc::now().timestamp()))
    }

    /// Reserves and arms the terminal request log, runs the probe, then cancels the
    /// reservation on failure or settles it on success.
    async fn execute_probe(
        &self,
        job: &ProbeJob<'_>,
        probe_http: &reqwest::Client,
    ) -> Option<ChannelProbeOutcome> {
        let ProbeJob {
            provider,
            channel,
            probe,
            ..
        } = job;
        let model_name = probe.logical_model;
        if !self.can_dispatch(job.due).await || self.scheduler.shutting_down() {
            return None;
        }
        let prepared = match prepare_channel_probe(
            &self.scheduler.state,
            probe_http,
            provider,
            channel,
            *probe,
            self.settings,
            job.stream,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                log_probe_result(provider, channel, model_name, false);
                return Some(error.into_outcome());
            }
        };
        if self.scheduler.shutting_down() {
            return None;
        }
        let user_store = &self.scheduler.state.user_store;
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
                return None;
            }
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let transformed_model = prepared
            .upstream_model()
            .filter(|model| *model != probe.upstream_model)
            .map(str::to_owned);
        let log_context = ActiveProbeLogContext {
            request_id: &request_id,
            user_id: &self.scheduler.user_id,
            provider_id: &provider.id,
            provider_name: &provider.name,
            provider_type: probe.provider_type,
            channel_multiplier: Some(probe.multiplier),
            channel_id: &channel.id,
            channel_name: &channel.name,
            logical_model: model_name,
            upstream_model: transformed_model.as_deref().unwrap_or(probe.upstream_model),
            is_stream: prepared.is_stream(),
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
            return None;
        }
        if !self.can_dispatch(job.due).await || self.scheduler.shutting_down() {
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
            return None;
        }
        let started_at = std::time::Instant::now();
        let mut outcome = prepared.send(job.timeout_ms).await;
        log_probe_result(provider, channel, model_name, outcome.ok);
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
                return None;
            }
            return Some(outcome);
        }
        if let Err(error) = persist_active_probe_request_log(
            user_store,
            log_context,
            self.pricing.resolve(probe.upstream_model, model_name),
            outcome.usage.take(),
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
            return None;
        }
        Some(outcome)
    }

    async fn update_health(&self, job: &ProbeJob<'_>, outcome: &ChannelProbeOutcome) {
        let mut health = self.scheduler.state.channel_health.lock().await;
        // Revision advancement and the trip check share the health lock with the write.
        if self
            .scheduler
            .state
            .routing_config_revision
            .load(Ordering::Acquire)
            != self.revision
        {
            return;
        }
        let Some(entry) = health.get_mut(&job.due.key) else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        if !Self::current_trip(entry, job.due, now) {
            return;
        }
        let wait = if outcome.ok {
            entry.probe_success_count = entry.probe_success_count.saturating_add(1);
            if entry.probe_success_count >= job.success_threshold {
                entry.mark_success(now);
                return;
            }
            job.interval
        } else {
            entry.probe_success_count = 0;
            recovery_wait(
                job.interval,
                job.rate_limit_wait,
                outcome.failure_class == Some(RetryableFailureClass::RateLimited),
                outcome.retry_after_seconds,
            )
        };
        entry.last_probe_at = Some(now);
        entry.next_recovery_at = Some(recovery_deadline(now, wait));
    }
}

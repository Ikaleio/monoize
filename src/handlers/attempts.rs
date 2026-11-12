use super::*;

/// Position of one dispatch within the routed Channel slots.
#[derive(Clone, Copy)]
pub(super) struct AttemptPosition {
    /// Request-wide upstream attempt number, starting at 1.
    pub(super) attempt_number: u32,
    /// Dispatches on this Channel so far, this one included.
    pub(super) channel_attempts_used: usize,
    /// FP6j-2: no later slot can follow, so this slot takes the pristine request.
    pub(super) final_slot: bool,
}

/// One dispatch to the Channel of `attempt`.
pub(super) struct AttemptSlot {
    pub(super) attempt: MonoizeAttempt,
    pub(super) position: AttemptPosition,
}

/// RTA-4/RTA-5 executor shared by every forwarding path. A caller runs its
/// transport between `next` and `fail`; a local error ends the request
/// without calling `fail`.
pub(super) struct AttemptDriver {
    candidates: std::vec::IntoIter<MonoizeAttempt>,
    /// A same-Channel retry queued by `fail`, with that Channel's dispatch count.
    retry: Option<(MonoizeAttempt, usize)>,
    /// RTA-4: dispatches per Provider, bounded by its total attempt budget.
    provider_attempts_used: HashMap<String, usize>,
    /// RTA-6c: `provider_id\0origin_key` of origins skipped after a shared-origin blast.
    shared_origin_skips: HashSet<String>,
    attempt_count: u32,
    dispatch_started: Option<Instant>,
    pub(super) tried_providers: Vec<TriedProvider>,
    pub(super) last_failed_attempt: Option<MonoizeAttempt>,
}

impl AttemptDriver {
    pub(super) fn new(attempts: Vec<MonoizeAttempt>) -> Self {
        Self {
            candidates: attempts.into_iter(),
            retry: None,
            provider_attempts_used: HashMap::new(),
            shared_origin_skips: HashSet::new(),
            attempt_count: 0,
            dispatch_started: None,
            tried_providers: Vec::new(),
            last_failed_attempt: None,
        }
    }

    /// The queued same-Channel retry, else the next candidate that neither
    /// budget nor shared-origin skip excludes.
    pub(super) fn next(&mut self) -> Option<AttemptSlot> {
        if let Some(slot) = self.next_retry() {
            return Some(slot);
        }
        loop {
            let attempt = self.candidates.next()?;
            if let Some(slot) = self.begin_slot(attempt, 0) {
                return Some(slot);
            }
        }
    }

    pub(super) fn next_retry(&mut self) -> Option<AttemptSlot> {
        let (attempt, used) = self.retry.take()?;
        self.begin_slot(attempt, used)
    }

    fn begin_slot(&mut self, attempt: MonoizeAttempt, used: usize) -> Option<AttemptSlot> {
        if self.should_skip(&attempt) {
            return None;
        }
        let channel_attempts_used = used + 1;
        let final_slot = self.candidates.len() == 0
            && channel_attempts_used == same_channel_attempt_slots(&attempt);
        let attempt_number = self.start_dispatch(&attempt);
        Some(AttemptSlot {
            attempt,
            position: AttemptPosition {
                attempt_number,
                channel_attempts_used,
                final_slot,
            },
        })
    }

    fn start_dispatch(&mut self, attempt: &MonoizeAttempt) -> u32 {
        self.dispatch_started = Some(Instant::now());
        *self
            .provider_attempts_used
            .entry(attempt.provider_id.clone())
            .or_default() += 1;
        self.attempt_count += 1;
        self.attempt_count
    }

    fn should_skip(&self, attempt: &MonoizeAttempt) -> bool {
        let budget_spent = attempt.provider_attempt_limit.is_some_and(|limit| {
            self.provider_attempts_used
                .get(&attempt.provider_id)
                .copied()
                .unwrap_or(0)
                >= limit
        });
        budget_spent
            || shared_origin_skip_token(attempt)
                .is_some_and(|token| self.shared_origin_skips.contains(&token))
    }

    /// Records one upstream failure and queues a same-Channel retry when
    /// RTA-4, RTA-4a, and RTA-5 allow one.
    pub(super) async fn fail(
        &mut self,
        state: &AppState,
        slot: AttemptSlot,
        error: &AppError,
        failure_class: Option<RetryableFailureClass>,
    ) {
        self.record_terminal_failure(state, &slot, error, failure_class)
            .await;
        if self
            .allows_same_channel_retry(state, &slot, failure_class)
            .await
        {
            let interval_ms = slot.attempt.channel_retry_interval_ms;
            if interval_ms > 0 {
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
            self.retry = Some((slot.attempt, slot.position.channel_attempts_used));
        }
    }

    /// Records a failed dispatch in `tried_providers`, Channel health, and the
    /// shared-origin skips without queuing a same-Channel retry.
    pub(super) async fn record_terminal_failure(
        &mut self,
        state: &AppState,
        slot: &AttemptSlot,
        error: &AppError,
        failure_class: Option<RetryableFailureClass>,
    ) {
        let mask_sensitive_info = state.monoize_runtime.read().await.mask_sensitive_info;
        self.tried_providers.push(TriedProvider::from_app_error(
            slot.position.attempt_number,
            &slot.attempt,
            error,
            self.dispatch_duration_ms(),
            mask_sensitive_info,
        ));
        if record_channel_failure(state, &slot.attempt, error, failure_class).await
            && let Some(token) = shared_origin_skip_token(&slot.attempt)
        {
            self.shared_origin_skips.insert(token);
        }
        self.last_failed_attempt = Some(slot.attempt.clone());
    }

    /// RTA-4a: the affinity-hit target earns one dispatch beyond
    /// `channel_max_retries + 1` for a Transient failure, so one transient
    /// fault on the bound Channel does not discard prompt-cache locality.
    async fn allows_same_channel_retry(
        &self,
        state: &AppState,
        slot: &AttemptSlot,
        failure_class: Option<RetryableFailureClass>,
    ) -> bool {
        let attempt = &slot.attempt;
        let limit = match failure_class {
            None | Some(RetryableFailureClass::Persistent) => return false,
            Some(RetryableFailureClass::Transient) if attempt.affinity_hit == Some(true) => {
                channel_attempt_limit(attempt) + 1
            }
            Some(_) => channel_attempt_limit(attempt),
        };
        slot.position.channel_attempts_used < limit
            && !self.should_skip(attempt)
            && is_attempt_channel_healthy(state, attempt).await
    }

    /// Image transport negotiation is another dispatch on the same slot,
    /// not a passive failure or a same-Channel retry.
    pub(super) fn negotiate(
        &mut self,
        slot: &mut AttemptSlot,
        error: &AppError,
        mask_sensitive_info: bool,
    ) {
        self.tried_providers.push(TriedProvider::from_app_error(
            slot.position.attempt_number,
            &slot.attempt,
            error,
            self.dispatch_duration_ms(),
            mask_sensitive_info,
        ));
        slot.position.attempt_number = self.start_dispatch(&slot.attempt);
    }

    fn dispatch_duration_ms(&self) -> Option<u64> {
        self.dispatch_started
            .map(|started| started.elapsed().as_millis() as u64)
    }

    /// RTA-8 plus SE1d: same-family routes also forward the final attempt's
    /// retry and rate-limit headers.
    pub(super) fn exhausted_error(
        &self,
        logical_model: &str,
        downstream: DownstreamProtocol,
    ) -> AppError {
        let mut err = build_exhausted_upstream_error(logical_model, &self.tried_providers);
        if self
            .last_failed_attempt
            .as_ref()
            .is_some_and(|attempt| downstream.is_same_family(attempt.provider_type))
            && let Some(last) = self.tried_providers.last()
        {
            err.forwarded_headers = last.forwarded_headers.clone();
        }
        err
    }
}

/// RTA-4: `channel_max_retries + 1` dispatches per Channel.
fn channel_attempt_limit(attempt: &MonoizeAttempt) -> usize {
    (attempt.channel_max_retries + 1).max(1) as usize
}

/// RTA-4/RTA-4a: the Channel's slot count, including the affinity-hit
/// target's extra slot that `allows_same_channel_retry` may grant.
fn same_channel_attempt_slots(attempt: &MonoizeAttempt) -> usize {
    channel_attempt_limit(attempt) + usize::from(attempt.affinity_hit == Some(true))
}

fn shared_origin_skip_token(attempt: &MonoizeAttempt) -> Option<String> {
    let origin_key = attempt.origin_key.as_deref()?;
    Some(format!("{}\0{origin_key}", attempt.provider_id))
}

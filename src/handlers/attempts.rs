use super::*;

/// Position of one dispatch within the routed Channel slots.
#[derive(Clone, Copy)]
pub(super) struct AttemptPosition {
    pub(super) attempt_number: u32,
    pub(super) channel_attempts_used: usize,
    pub(super) final_slot: bool,
}

/// One owned Channel dispatch; retry returns its updated routing metadata.
pub(super) struct AttemptSlot {
    pub(super) attempt: MonoizeAttempt,
    pub(super) position: AttemptPosition,
}

/// Shared RTA-4/RTA-5 executor state. Transport-specific work runs between
/// `next` and `fail`; local failures end the request without calling `fail`.
pub(super) struct AttemptDriver {
    candidates: std::vec::IntoIter<MonoizeAttempt>,
    retry: Option<(MonoizeAttempt, usize)>,
    execution: AttemptExecutionState,
    pub(super) tried_providers: Vec<TriedProvider>,
    pub(super) last_failed_attempt: Option<MonoizeAttempt>,
}

impl AttemptDriver {
    pub(super) fn new(attempts: Vec<MonoizeAttempt>) -> Self {
        Self {
            candidates: attempts.into_iter(),
            retry: None,
            execution: AttemptExecutionState::default(),
            tried_providers: Vec::new(),
            last_failed_attempt: None,
        }
    }

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
        if self.execution.should_skip(&attempt) {
            return None;
        }
        let channel_attempts_used = used + 1;
        let final_slot = self.candidates.len() == 0
            && channel_attempts_used == same_channel_attempt_slots(&attempt);
        let attempt_number = self.execution.record_upstream_attempt(&attempt);
        Some(AttemptSlot {
            attempt,
            position: AttemptPosition {
                attempt_number,
                channel_attempts_used,
                final_slot,
            },
        })
    }

    /// Records one upstream failure and queues a same-Channel retry only
    /// when its budget, health, affinity, and failure class allow it.
    pub(super) async fn fail(
        &mut self,
        state: &AppState,
        slot: AttemptSlot,
        error: &AppError,
        failure_class: Option<RetryableFailureClass>,
    ) -> bool {
        self.record_failure(state, &slot, error, failure_class)
            .await;
        let retry = allow_same_channel_retry(
            state,
            &slot.attempt,
            &self.execution,
            slot.position.channel_attempts_used,
            failure_class,
        )
        .await;
        if retry {
            maybe_sleep_before_channel_retry(&slot.attempt).await;
            self.retry = Some((slot.attempt, slot.position.channel_attempts_used));
        }
        retry
    }

    /// A failure after downstream output still records health and tried
    /// metadata, but must not authorize another dispatch.
    pub(super) async fn record_terminal_failure(
        &mut self,
        state: &AppState,
        slot: &AttemptSlot,
        error: &AppError,
        failure_class: Option<RetryableFailureClass>,
    ) {
        self.record_failure(state, slot, error, failure_class).await;
    }

    async fn record_failure(
        &mut self,
        state: &AppState,
        slot: &AttemptSlot,
        error: &AppError,
        failure_class: Option<RetryableFailureClass>,
    ) {
        record_upstream_attempt_failure(
            state,
            &slot.attempt,
            slot.position.attempt_number,
            error,
            failure_class,
            &mut self.tried_providers,
            &mut self.execution,
        )
        .await;
        self.last_failed_attempt = Some(slot.attempt.clone());
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
            self.execution.last_attempt_duration_ms(),
            mask_sensitive_info,
        ));
        slot.position.attempt_number = self.execution.record_upstream_attempt(&slot.attempt);
    }

    pub(super) fn exhausted_error(
        &self,
        logical_model: &str,
        downstream: DownstreamProtocol,
    ) -> AppError {
        build_exhausted_error_for_downstream(
            logical_model,
            &self.tried_providers,
            self.last_failed_attempt.as_ref(),
            downstream,
        )
    }
}

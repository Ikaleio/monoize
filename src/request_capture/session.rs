use super::*;

impl RequestCaptureSession {
    pub(crate) async fn push_attempt(&self, mut attempt: Value) {
        let mut attempts = self.attempts.lock().await;
        let mut truncation = self.truncation.lock().await;
        if attempts.len() >= self.limits.max_attempts {
            truncation.omitted_attempts = truncation.omitted_attempts.saturating_add(1);
            truncation.omitted_frames = truncation.omitted_frames.saturating_add(
                attempt
                    .get("downstream_sse_frames")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            );
            truncation.omitted_bytes = truncation
                .omitted_bytes
                .saturating_add(serialized_len(&attempt));
            return;
        }
        let mut additionally_omitted_frames = 0_usize;
        let mut additionally_omitted_bytes = 0_usize;
        let retained_frame_count = if let Some(frames) = attempt
            .get_mut("downstream_sse_frames")
            .and_then(Value::as_array_mut)
        {
            let available = self
                .limits
                .max_frames
                .saturating_sub(truncation.retained_frames);
            if frames.len() > available {
                let omitted = frames.split_off(available);
                additionally_omitted_frames = omitted.len();
                additionally_omitted_bytes = omitted
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::len)
                    .sum::<usize>();
            }
            frames.len()
        } else {
            0
        };
        if additionally_omitted_frames > 0
            && let Some(metadata) = attempt
                .get_mut("downstream_sse_frames_truncation")
                .and_then(Value::as_object_mut)
        {
            let prior_frames = metadata
                .get("omitted_frames")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let prior_bytes = metadata
                .get("omitted_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            metadata.insert(
                "omitted_frames".to_string(),
                json!(prior_frames.saturating_add(additionally_omitted_frames as u64)),
            );
            metadata.insert(
                "omitted_bytes".to_string(),
                json!(prior_bytes.saturating_add(additionally_omitted_bytes as u64)),
            );
            metadata.insert("truncated".to_string(), Value::Bool(true));
        }
        if let Some(frame_meta) = attempt.get("downstream_sse_frames_truncation") {
            truncation.omitted_frames = truncation.omitted_frames.saturating_add(
                frame_meta
                    .get("omitted_frames")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
            );
            truncation.omitted_bytes = truncation.omitted_bytes.saturating_add(
                frame_meta
                    .get("omitted_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
            );
        }
        let attempt_bytes = serialized_len(&attempt);
        let remaining = self
            .limits
            .max_session_bytes
            .saturating_sub(truncation.retained_bytes);
        let retained_attempt = if attempt_bytes <= remaining {
            truncation.retained_frames = truncation
                .retained_frames
                .saturating_add(retained_frame_count);
            attempt
        } else {
            truncation.omitted_frames = truncation
                .omitted_frames
                .saturating_add(retained_frame_count);
            let placeholder = truncated_attempt_placeholder(&attempt, attempt_bytes);
            let placeholder_bytes = serialized_len(&placeholder);
            truncation.omitted_bytes = truncation
                .omitted_bytes
                .saturating_add(attempt_bytes.saturating_sub(placeholder_bytes.min(attempt_bytes)));
            if placeholder_bytes > remaining {
                truncation.omitted_attempts = truncation.omitted_attempts.saturating_add(1);
                truncation.omitted_bytes =
                    truncation.omitted_bytes.saturating_add(placeholder_bytes);
                return;
            }
            placeholder
        };
        truncation.retained_bytes = truncation
            .retained_bytes
            .saturating_add(serialized_len(&retained_attempt));
        attempts.push(retained_attempt);
    }

    /// RCD-Z3: bounding and envelope encoding run synchronously here;
    /// compression, the file write, the metadata insert, and the RCD-R8
    /// size-budget step run in the returned background task. Request paths
    /// ignore the handle; callers that need synchronous completion may await it.
    pub(crate) async fn persist_with_result(
        &self,
        upstream_usage: Option<&crate::urp::Usage>,
        upstream_error_seen: bool,
    ) -> Option<JoinHandle<()>> {
        let mut attempts = self.attempts.lock().await.clone();
        if attempts.is_empty() {
            return None;
        }
        let mut truncation = self.truncation.lock().await.clone();
        let multiple_upstream_attempts =
            attempts.len().saturating_add(truncation.omitted_attempts) > 1;
        if !self.mode.should_persist(
            upstream_usage,
            upstream_error_seen,
            multiple_upstream_attempts,
        ) {
            return None;
        }
        let encoded = self.encode_bounded_attempts(&mut attempts, &mut truncation)?;
        // RCD-Z3 step 2: everything from compression onward is detached from
        // the HTTP response path.
        let store = self.store.clone();
        let request_id = self.request_id.clone();
        let created_at = self.created_at;
        let user_id = self.user_id.clone();
        let api_key_id = self.api_key_id.clone();
        // RCD-R1: the TTL deadline is fixed from the retention resolved at
        // request-authentication time, not at write-completion time.
        let expires_at_unix_ms = created_at.timestamp_millis().saturating_add(
            i64::try_from(self.retention.duration_seconds().saturating_mul(1000))
                .unwrap_or(i64::MAX),
        );
        Some(tokio::spawn(async move {
            let (file_name, size_bytes) = match store
                .write_dump(request_id.as_deref(), created_at, encoded)
                .await
            {
                Ok(written) => written,
                Err(err) => {
                    tracing::warn!("failed to write request capture dump: {err}");
                    return;
                }
            };
            // RCD-M3: only sessions with a request id get a metadata row;
            // RCD-M4: an insert failure keeps the dump file and the client
            // response intact.
            let Some(request_id) = request_id else {
                return;
            };
            let record = CaptureRecordRow {
                file_name,
                request_id,
                user_id,
                api_key_id,
                created_at: created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
                created_at_unix_ms: created_at.timestamp_millis(),
                size_bytes,
                expires_at_unix_ms,
            };
            if let Err(err) = store.insert_capture_record(&record).await {
                tracing::warn!("failed to record request capture metadata: {err}");
                return;
            }
            // RCD-R8: bound budget overshoot to the window between this write
            // and the completion of this step.
            if let Err(err) = store.enforce_size_budget().await {
                tracing::warn!("request capture size rotation failed: {err}");
            }
        }))
    }

    fn encode_bounded_attempts(
        &self,
        attempts: &mut Vec<Value>,
        truncation: &mut CaptureTruncation,
    ) -> Option<Vec<u8>> {
        loop {
            let payload = json!({
                "version": 2,
                "request_id": self.request_id,
                "created_at": self.created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
                "api_key_id": self.api_key_id,
                "user_id": self.user_id,
                "downstream_protocol": downstream_protocol_name(self.downstream_protocol),
                "is_stream": self.is_stream,
                "attempts": &*attempts,
                "capture_truncation": truncation.to_json(),
            });
            let encoded = match serde_json::to_vec(&payload) {
                Ok(encoded) => encoded,
                Err(error) => {
                    tracing::warn!("failed to encode request capture dump: {error}");
                    return None;
                }
            };
            if encoded.len() <= self.limits.max_session_bytes {
                return Some(encoded);
            }
            if attempts.len() == 1 {
                let original_bytes = serialized_len(&attempts[0]);
                let placeholder = truncated_attempt_placeholder(&attempts[0], original_bytes);
                let placeholder_bytes = serialized_len(&placeholder);
                if placeholder_bytes >= original_bytes {
                    tracing::warn!(
                        max_session_bytes = self.limits.max_session_bytes,
                        "bounded request capture envelope exceeds configured session byte limit"
                    );
                    return None;
                }
                attempts[0] = placeholder;
                truncation.omitted_bytes = truncation
                    .omitted_bytes
                    .saturating_add(original_bytes.saturating_sub(placeholder_bytes));
                truncation.retained_bytes = truncation
                    .retained_bytes
                    .saturating_sub(original_bytes)
                    .saturating_add(placeholder_bytes);
                continue;
            }
            let Some(removed) = attempts.pop() else {
                tracing::warn!(
                    max_session_bytes = self.limits.max_session_bytes,
                    "request capture envelope exceeds configured session byte limit"
                );
                return None;
            };
            let removed_bytes = serialized_len(&removed);
            truncation.omitted_attempts = truncation.omitted_attempts.saturating_add(1);
            truncation.omitted_bytes = truncation.omitted_bytes.saturating_add(removed_bytes);
            truncation.retained_bytes = truncation.retained_bytes.saturating_sub(removed_bytes);
        }
    }
}

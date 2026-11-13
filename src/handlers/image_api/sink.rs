use super::*;

/// Downstream SSE event family for one streaming Image API request
/// (`image-api-proxy.spec.md` §1).
#[derive(Clone, Copy)]
pub(super) enum ImageStreamEventFamily {
    Generation,
    Edit,
}

impl ImageStreamEventFamily {
    fn partial_event_name(self) -> &'static str {
        match self {
            Self::Generation => "image_generation.partial_image",
            Self::Edit => "image_edit.partial_image",
        }
    }

    fn completed_event_name(self) -> &'static str {
        match self {
            Self::Generation => "image_generation.completed",
            Self::Edit => "image_edit.completed",
        }
    }
}

/// Downstream frame sink for one §5.5 streaming Image API sub-request. Send
/// failures are ignored: a disconnected client must not fail the upstream
/// collection, billing, or logging that the executor still has to finish.
pub(super) struct ImageStreamSink {
    family: ImageStreamEventFamily,
    tx: mpsc::Sender<Event>,
    partial_frames_emitted: u64,
    completed_frames_emitted: u64,
}

impl ImageStreamSink {
    pub(super) fn new(family: ImageStreamEventFamily, tx: mpsc::Sender<Event>) -> Self {
        Self {
            family,
            tx,
            partial_frames_emitted: 0,
            completed_frames_emitted: 0,
        }
    }

    /// IS7: true once any partial or completed frame reached the client.
    fn frames_emitted(&self) -> bool {
        self.partial_frames_emitted > 0 || self.completed_frames_emitted > 0
    }

    async fn send_frame(&self, event_name: &str, payload: Value) -> AppResult<()> {
        crate::urp::stream_helpers::send_named_sse_json(&self.tx, event_name, payload).await
    }

    /// IS3: one partial frame per canonical Base64 image delta.
    pub(super) async fn send_partial(
        &mut self,
        source: &urp::ImageSource,
        extra_body: &HashMap<String, Value>,
    ) -> AppResult<()> {
        let urp::ImageSource::Base64 { data, .. } = source else {
            return Ok(());
        };
        let event_name = self.family.partial_event_name();
        let mut payload = Map::new();
        payload.insert("type".to_string(), Value::String(event_name.to_string()));
        payload.insert("b64_json".to_string(), Value::String(data.clone()));
        let index = extra_body
            .get("partial_image_index")
            .and_then(Value::as_u64)
            .unwrap_or(self.partial_frames_emitted);
        payload.insert("partial_image_index".to_string(), Value::from(index));
        let created_at = extra_body
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| chrono::Utc::now().timestamp());
        payload.insert("created_at".to_string(), Value::from(created_at));
        for key in ["output_format", "size", "quality", "background"] {
            if let Some(value) = extra_body.get(key) {
                payload.insert(key.to_string(), value.clone());
            }
        }
        self.partial_frames_emitted += 1;
        self.send_frame(event_name, Value::Object(payload)).await
    }

    /// IS4: one completed frame per extracted image; the last frame carries
    /// `usage` iff the sub-request produced URP usage.
    pub(super) async fn send_completed(&mut self, resp: &urp::UrpResponse) -> AppResult<()> {
        let images = extract_images_from_response(resp);
        let total = images.len();
        for (position, image) in images.into_iter().enumerate() {
            let event_name = self.family.completed_event_name();
            let mut payload = Map::new();
            payload.extend(image.generation.common_response_fields());
            if let Some(revised_prompt) = image.revised_prompt {
                payload.insert("revised_prompt".to_string(), Value::String(revised_prompt));
            }
            payload.insert("type".to_string(), Value::String(event_name.to_string()));
            if let Some(b64) = image.b64_json {
                payload.insert("b64_json".to_string(), Value::String(b64));
            } else if let Some(url) = image.url {
                payload.insert("url".to_string(), Value::String(url));
            }
            if let Some(mime_type) = image.mime_type {
                payload.insert("mime_type".to_string(), Value::String(mime_type));
            }
            payload.insert(
                "created_at".to_string(),
                Value::from(chrono::Utc::now().timestamp()),
            );
            if position + 1 == total
                && let Some(usage) = &resp.usage
            {
                let mut aggregated = AggregatedUsage::default();
                accumulate_usage(&mut aggregated, usage);
                payload.insert("usage".to_string(), aggregated_usage_value(&aggregated));
            }
            self.completed_frames_emitted += 1;
            self.send_frame(event_name, Value::Object(payload)).await?;
        }
        Ok(())
    }

    /// IS6: exactly one `error` frame with the standard error object.
    pub(super) async fn send_error(&mut self, err: &AppError) {
        let _ = self
            .send_frame(
                "error",
                json!({
                    "type": "error",
                    "error": {
                        "message": err.message,
                        "type": err.error_type,
                        "code": err.code,
                        "param": err.param,
                    }
                }),
            )
            .await;
    }

    /// IS5: `data: [DONE]` terminator.
    pub(super) async fn send_done(&self) {
        let _ = self.tx.send(Event::plain("[DONE]")).await;
    }
}

/// IS7 guard used at every attempt-failover decision inside the streaming
/// executor.
pub(super) fn sink_frames_emitted(sink: &Option<&mut ImageStreamSink>) -> bool {
    sink.as_ref().is_some_and(|sink| sink.frames_emitted())
}

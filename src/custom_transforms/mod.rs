//! Custom JS transforms (`custom-js-transforms.spec.md`): administrator-authored
//! JavaScript transforms executed in a QuickJS sandbox, persisted in the
//! `custom_transforms` table, and resolved dynamically in the URP v2 pipeline.

pub mod frontmatter;
pub mod sandbox;
pub mod store;

pub use frontmatter::{
    CUSTOM_TRANSFORM_ID_PREFIX, CustomTransformMeta, CustomTransformVisibility,
    is_valid_custom_transform_id, parse_frontmatter,
};
pub use sandbox::SandboxLimits;
pub use store::{
    CustomTransformEntry, CustomTransformError, CustomTransformRecord, CustomTransformSnapshot,
    CustomTransformSnapshotHandle, CustomTransformStore, default_config_schema,
};

use crate::transforms::{
    DynTransform, Phase, TransformError, TransformRuntimeContext, TransformState, UrpData,
};
use crate::urp::UrpStreamEvent;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::any::Any;

/// Per-rule state for one custom transform within one request (CJS-JS-5).
/// `js_state` is the JSON value handed to the script as `ctx.state`;
/// `pending_stream` carries the CJS-JS-6 disposition from `apply` to
/// `finalize_stream_event`.
pub struct CustomJsState {
    js_state: Value,
    pending_stream: StreamDisposition,
}

enum StreamDisposition {
    Keep,
    Drop,
    Replace(Vec<UrpStreamEvent>),
}

impl CustomJsState {
    fn new() -> Self {
        Self {
            js_state: json!({}),
            pending_stream: StreamDisposition::Keep,
        }
    }
}

impl TransformState for CustomJsState {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn finalize_stream_event(&mut self, event: UrpStreamEvent) -> Vec<UrpStreamEvent> {
        match std::mem::replace(&mut self.pending_stream, StreamDisposition::Keep) {
            StreamDisposition::Keep => vec![event],
            StreamDisposition::Drop => Vec::new(),
            StreamDisposition::Replace(events) => events,
        }
    }
}

#[async_trait]
impl DynTransform for CustomTransformEntry {
    fn declared_phases(&self) -> &[Phase] {
        &self.phases
    }

    fn init_state(&self) -> Box<dyn TransformState> {
        Box::new(CustomJsState::new())
    }

    async fn apply(
        &self,
        data: UrpData<'_>,
        phase: Phase,
        context: &TransformRuntimeContext,
        config: &Value,
        state: &mut dyn TransformState,
    ) -> Result<(), TransformError> {
        let state = state
            .as_any_mut()
            .downcast_mut::<CustomJsState>()
            .ok_or_else(|| {
                TransformError::Apply(format!(
                    "custom transform '{}' received a foreign state type",
                    self.id
                ))
            })?;

        let apply_error = |detail: String| {
            TransformError::Apply(format!("custom transform '{}': {detail}", self.id))
        };

        let (kind, payload) = match &data {
            UrpData::Request(request) => ("request", serde_json::to_value(&**request)),
            UrpData::Response(response) => ("response", serde_json::to_value(&**response)),
            UrpData::Stream(event) => ("stream", serde_json::to_value(&**event)),
        };
        let payload =
            payload.map_err(|error| apply_error(format!("serialize payload: {error}")))?;

        let invocation = sandbox::SandboxInvocation {
            transform_id: self.id.clone(),
            source: self.source.clone(),
            kind,
            phase,
            data: payload,
            config: config.clone(),
            state: state.js_state.clone(),
            upstream_provider_type: context
                .upstream_provider_type
                .and_then(|provider_type| serde_json::to_value(provider_type).ok())
                .and_then(|value| value.as_str().map(str::to_string)),
        };

        let outcome = sandbox::run_transform(
            invocation,
            Some(context.http_client.clone()),
            SandboxLimits::from_env(),
        )
        .await
        .map_err(apply_error)?;

        state.js_state = outcome.state;

        match (data, outcome.data) {
            (UrpData::Request(request), sandbox::SandboxData::Single(value)) => {
                *request = serde_json::from_value(value).map_err(|error| {
                    apply_error(format!("result is not a valid UrpRequest: {error}"))
                })?;
            }
            (UrpData::Response(response), sandbox::SandboxData::Single(value)) => {
                *response = serde_json::from_value(value).map_err(|error| {
                    apply_error(format!("result is not a valid UrpResponse: {error}"))
                })?;
            }
            (UrpData::Stream(event), sandbox::SandboxData::Single(value)) => {
                *event = serde_json::from_value(value).map_err(|error| {
                    apply_error(format!("result is not a valid UrpStreamEvent: {error}"))
                })?;
                state.pending_stream = StreamDisposition::Keep;
            }
            (UrpData::Stream(_), sandbox::SandboxData::Dropped) => {
                state.pending_stream = StreamDisposition::Drop;
            }
            (UrpData::Stream(_), sandbox::SandboxData::Fanout(values)) => {
                let events = values
                    .into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<Vec<UrpStreamEvent>, _>>()
                    .map_err(|error| {
                        apply_error(format!(
                            "fan-out element is not a valid UrpStreamEvent: {error}"
                        ))
                    })?;
                state.pending_stream = StreamDisposition::Replace(events);
            }
            // The sandbox rejects Dropped/Fanout for non-stream kinds
            // (CJS-JS-4), so these arms are unreachable but kept as errors.
            (UrpData::Request(_) | UrpData::Response(_), _) => {
                return Err(apply_error(
                    "non-stream invocation produced a stream-only disposition".to_string(),
                ));
            }
        }
        Ok(())
    }
}

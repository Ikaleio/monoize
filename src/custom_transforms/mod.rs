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

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::image_transform_cache::ImageTransformCache;
    use crate::transforms::{
        TransformResolver, TransformRuleConfig, TransformScope, apply_stream_transforms,
        apply_transforms, build_states_for_rules, registry,
    };
    use crate::urp::UrpRequest;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn context() -> TransformRuntimeContext {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache = ImageTransformCache::new(
            temp_dir.path().join("cache"),
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("cache");
        TransformRuntimeContext {
            image_transform_cache: Arc::new(cache),
            http_client: reqwest::Client::new(),
            upstream_provider_type: None,
        }
    }

    fn snapshot_with(source: &str, id: &str, phases: Vec<Phase>) -> CustomTransformSnapshot {
        CustomTransformSnapshot::from_entries(
            [(
                id.to_string(),
                Arc::new(CustomTransformEntry {
                    id: id.to_string(),
                    name: "n".to_string(),
                    description: "d".to_string(),
                    author: "a".to_string(),
                    source: source.to_string(),
                    visibility: CustomTransformVisibility::User,
                    phases,
                    scopes: vec![TransformScope::Provider],
                    config_schema: None,
                }),
            )]
            .into_iter()
            .collect(),
        )
    }

    fn rule(id: &str, phase: Phase, config: serde_json::Value) -> TransformRuleConfig {
        TransformRuleConfig {
            transform: id.to_string(),
            enabled: true,
            models: None,
            phase,
            config,
        }
    }

    fn request(model: &str) -> UrpRequest {
        UrpRequest {
            model: model.to_string(),
            input: Vec::new(),
            stream: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            stop: None,
            verbosity: None,
            response_format: None,
            user: None,
            extra_body: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn custom_transform_rewrites_request_through_pipeline() {
        let builtin = registry();
        let snapshot = snapshot_with(
            "function transform(ctx) { ctx.data.model = ctx.config.target; }",
            "js:model-rewrite",
            vec![Phase::Request],
        );
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule(
            "js:model-rewrite",
            Phase::Request,
            json!({"target": "rewritten-model"}),
        )];
        let mut states = build_states_for_rules(&rules, resolver).expect("states");
        let mut req = request("original-model");
        apply_transforms(
            UrpData::Request(&mut req),
            &rules,
            &mut states,
            "original-model",
            Phase::Request,
            &context().await,
            resolver,
        )
        .await
        .expect("apply");
        assert_eq!(req.model, "rewritten-model");
    }

    /// CJS-RT-5: the rule phase must also be in the declared phases.
    #[tokio::test]
    async fn custom_transform_outside_declared_phases_is_a_noop() {
        let builtin = registry();
        let snapshot = snapshot_with(
            "function transform(ctx) { ctx.data.model = 'must-not-happen'; }",
            "js:request-only",
            vec![Phase::Request],
        );
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule("js:request-only", Phase::Response, json!({}))];
        let mut states = build_states_for_rules(&rules, resolver).expect("states");
        let mut req = request("original-model");
        apply_transforms(
            UrpData::Request(&mut req),
            &rules,
            &mut states,
            "original-model",
            Phase::Response,
            &context().await,
            resolver,
        )
        .await
        .expect("apply");
        assert_eq!(req.model, "original-model");
    }

    /// CJS-RT-3: unresolved `js:` rules never fail the request.
    #[tokio::test]
    async fn unresolved_custom_rule_is_skipped_without_error() {
        let builtin = registry();
        let snapshot = CustomTransformSnapshot::default();
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule("js:deleted-transform", Phase::Request, json!({}))];
        let mut states = build_states_for_rules(&rules, resolver).expect("states");
        let mut req = request("original-model");
        apply_transforms(
            UrpData::Request(&mut req),
            &rules,
            &mut states,
            "original-model",
            Phase::Request,
            &context().await,
            resolver,
        )
        .await
        .expect("apply must not fail");
        assert_eq!(req.model, "original-model");
    }

    /// CJS-RT-4: unknown non-`js:` IDs keep the existing not-found error.
    #[tokio::test]
    async fn unknown_builtin_rule_still_fails() {
        let builtin = registry();
        let snapshot = CustomTransformSnapshot::default();
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule("no_such_transform", Phase::Request, json!({}))];
        assert!(build_states_for_rules(&rules, resolver).is_err());
    }

    #[tokio::test]
    async fn script_error_surfaces_as_apply_error_without_crash() {
        let builtin = registry();
        let snapshot = snapshot_with(
            "function transform(ctx) { throw new Error('kaboom'); }",
            "js:throws",
            vec![Phase::Request],
        );
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule("js:throws", Phase::Request, json!({}))];
        let mut states = build_states_for_rules(&rules, resolver).expect("states");
        let mut req = request("original-model");
        let error = apply_transforms(
            UrpData::Request(&mut req),
            &rules,
            &mut states,
            "original-model",
            Phase::Request,
            &context().await,
            resolver,
        )
        .await
        .expect_err("must fail");
        assert!(error.to_string().contains("kaboom"), "got: {error}");
    }

    #[tokio::test]
    async fn stream_transform_drops_and_fans_out_events() {
        let builtin = registry();
        // Drops response_start events; duplicates response_done events.
        let source = r#"function transform(ctx) {
          if (ctx.data.event === "response_start") return null;
          if (ctx.data.event === "response_done") return [ctx.data, ctx.data];
        }"#;
        let snapshot = snapshot_with(source, "js:stream-shaper", vec![Phase::Response]);
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule("js:stream-shaper", Phase::Response, json!({}))];
        let mut states = build_states_for_rules(&rules, resolver).expect("states");
        let context = context().await;

        let start = UrpStreamEvent::ResponseStart {
            id: "resp_1".to_string(),
            model: "m".to_string(),
            extra_body: HashMap::new(),
        };
        let dropped = apply_stream_transforms(
            start,
            &rules,
            &mut states,
            "m",
            Phase::Response,
            &context,
            resolver,
        )
        .await
        .expect("apply");
        assert!(dropped.is_empty(), "response_start must be dropped");

        let done = UrpStreamEvent::ResponseDone {
            finish_reason: None,
            usage: None,
            output: Vec::new(),
            extra_body: HashMap::new(),
        };
        let fanned = apply_stream_transforms(
            done,
            &rules,
            &mut states,
            "m",
            Phase::Response,
            &context,
            resolver,
        )
        .await
        .expect("apply");
        assert_eq!(fanned.len(), 2, "response_done must fan out to two events");
    }

    /// CJS-JS-5: `ctx.state` persists across stream events of one request.
    #[tokio::test]
    async fn stream_state_persists_across_events() {
        let builtin = registry();
        let source = r#"function transform(ctx) {
          ctx.state.count = (ctx.state.count || 0) + 1;
          ctx.data.event_index = ctx.state.count;
        }"#;
        let snapshot = snapshot_with(source, "js:counter", vec![Phase::Response]);
        let resolver = TransformResolver::new(&builtin, &snapshot);
        let rules = vec![rule("js:counter", Phase::Response, json!({}))];
        let mut states = build_states_for_rules(&rules, resolver).expect("states");
        let context = context().await;

        for expected_index in 1..=3u64 {
            let event = UrpStreamEvent::ResponseStart {
                id: "resp".to_string(),
                model: "m".to_string(),
                extra_body: HashMap::new(),
            };
            let events = apply_stream_transforms(
                event,
                &rules,
                &mut states,
                "m",
                Phase::Response,
                &context,
                resolver,
            )
            .await
            .expect("apply");
            assert_eq!(events.len(), 1);
            let UrpStreamEvent::ResponseStart { extra_body, .. } = &events[0] else {
                panic!("expected response_start");
            };
            assert_eq!(extra_body["event_index"], json!(expected_index));
        }
    }
}

use super::*;
use crate::transforms::stream_split_sse_frames::DEFAULT_MAX_FRAME_LENGTH;

pub(in crate::handlers) async fn apply_transform_rules_request(
    state: &AppState,
    req: &mut urp::UrpRequest,
    rules: &[TransformRuleConfig],
    match_model: &str,
    upstream_provider_type: Option<ProviderType>,
) -> AppResult<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let custom_snapshot = state.custom_transform_store.snapshot();
    let resolver = transforms::TransformResolver::new(
        state.transform_registry.as_ref(),
        custom_snapshot.as_ref(),
    );
    let mut states = transforms::build_states_for_rules(rules, resolver).map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_init_failed",
            e.to_string(),
        )
    })?;
    let context = transforms::TransformRuntimeContext {
        image_transform_cache: state.image_transform_cache.clone(),
        http_client: state.http.clone(),
        upstream_provider_type,
    };
    transforms::apply_transforms(
        transforms::UrpData::Request(req),
        rules,
        &mut states,
        match_model,
        Phase::Request,
        &context,
        resolver,
    )
    .await
    .map_err(|e| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "transform_apply_failed",
            e.to_string(),
        )
    })
}

pub(in crate::handlers) async fn apply_transform_rules_response(
    state: &AppState,
    resp: &mut urp::UrpResponse,
    rules: &[TransformRuleConfig],
    model: &str,
    upstream_provider_type: Option<ProviderType>,
) -> AppResult<()> {
    if !rules.is_empty() {
        let custom_snapshot = state.custom_transform_store.snapshot();
        let resolver = transforms::TransformResolver::new(
            state.transform_registry.as_ref(),
            custom_snapshot.as_ref(),
        );
        let mut states = transforms::build_states_for_rules(rules, resolver).map_err(|e| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_init_failed",
                e.to_string(),
            )
        })?;
        let context = transforms::TransformRuntimeContext {
            image_transform_cache: state.image_transform_cache.clone(),
            http_client: state.http.clone(),
            upstream_provider_type,
        };
        transforms::apply_transforms(
            transforms::UrpData::Response(resp),
            rules,
            &mut states,
            model,
            Phase::Response,
            &context,
            resolver,
        )
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_apply_failed",
                e.to_string(),
            )
        })?;
    }
    Ok(())
}

pub(in crate::handlers) struct StreamTransformConfig<'a> {
    pub provider_rules: &'a [TransformRuleConfig],
    pub global_rules: &'a [TransformRuleConfig],
    pub auth_rules: &'a [TransformRuleConfig],
    pub model: &'a str,
    pub upstream_provider_type: Option<ProviderType>,
    pub reasoning_envelope: Option<(&'a str, &'a str)>,
}

struct StreamTransformStage<'a> {
    rules: &'a [TransformRuleConfig],
    states: Vec<Box<dyn transforms::TransformState>>,
}

impl<'a> StreamTransformStage<'a> {
    fn new(
        rules: &'a [TransformRuleConfig],
        resolver: transforms::TransformResolver<'_>,
    ) -> AppResult<Self> {
        let states = transforms::build_states_for_rules(rules, resolver).map_err(|error| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_init_failed",
                error.to_string(),
            )
        })?;
        Ok(Self { rules, states })
    }

    async fn apply(
        &mut self,
        event: urp::UrpStreamEvent,
        model: &str,
        context: &transforms::TransformRuntimeContext,
        resolver: transforms::TransformResolver<'_>,
    ) -> AppResult<Vec<urp::UrpStreamEvent>> {
        transforms::apply_stream_transforms(
            event,
            self.rules,
            &mut self.states,
            model,
            Phase::Response,
            context,
            resolver,
        )
        .await
        .map_err(|error| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_apply_failed",
                error.to_string(),
            )
        })
    }
}

pub(in crate::handlers) async fn transform_urp_stream(
    state: &AppState,
    mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
    tx: mpsc::Sender<urp::UrpStreamEvent>,
    config: StreamTransformConfig<'_>,
) -> AppResult<()> {
    // Keep the snapshot for the whole stream so all events use the same custom transforms.
    let custom_snapshot = state.custom_transform_store.snapshot();
    let resolver = transforms::TransformResolver::new(
        state.transform_registry.as_ref(),
        custom_snapshot.as_ref(),
    );
    let mut provider = StreamTransformStage::new(config.provider_rules, resolver)?;
    let mut global = StreamTransformStage::new(config.global_rules, resolver)?;
    let mut auth = StreamTransformStage::new(config.auth_rules, resolver)?;
    let context = transforms::TransformRuntimeContext {
        image_transform_cache: state.image_transform_cache.clone(),
        http_client: state.http.clone(),
        upstream_provider_type: config.upstream_provider_type,
    };
    let mut reasoning_envelope_state = urp::ReasoningEnvelopeStreamState::default();
    while let Some(event) = rx.recv().await {
        // Assemble fragments before wrapping so transforms never observe raw fragments
        // or a concatenation of independently wrapped envelopes.
        let enveloped_events = match config.reasoning_envelope {
            Some((provider_type, upstream_model)) => {
                reasoning_envelope_state.wrap_event(event, provider_type, upstream_model)
            }
            None => vec![event],
        };
        for event in enveloped_events {
            for provider_event in provider
                .apply(event, config.model, &context, resolver)
                .await?
            {
                for global_event in global
                    .apply(provider_event, config.model, &context, resolver)
                    .await?
                {
                    for auth_event in auth
                        .apply(global_event, config.model, &context, resolver)
                        .await?
                    {
                        tx.send(auth_event).await.map_err(|_| {
                            AppError::new(
                                StatusCode::BAD_GATEWAY,
                                "stream_transform_failed",
                                "failed to forward transformed stream event",
                            )
                        })?;
                    }
                }
            }
        }
    }
    Ok(())
}

pub(in crate::handlers) fn effective_sse_max_frame_length(
    provider_rules: &[TransformRuleConfig],
    global_rules: &[TransformRuleConfig],
    auth_rules: &[TransformRuleConfig],
    model: &str,
) -> Option<usize> {
    resolve_sse_max_frame_length_from_rules(provider_rules, model)
        .or_else(|| resolve_sse_max_frame_length_from_rules(global_rules, model))
        .or_else(|| resolve_sse_max_frame_length_from_rules(auth_rules, model))
}

fn resolve_sse_max_frame_length_from_rules(
    rules: &[TransformRuleConfig],
    model: &str,
) -> Option<usize> {
    rules
        .iter()
        .find(|rule| {
            rule.enabled
                && rule.phase == Phase::Response
                && rule.transform == "stream_split_sse_frames"
                && match &rule.models {
                    None => true,
                    Some(patterns) => patterns
                        .iter()
                        .any(|pattern| model_glob_match(pattern, model)),
                }
        })
        .map(|rule| {
            rule.config
                .get("max_frame_length")
                .and_then(|v| v.as_u64())
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_MAX_FRAME_LENGTH)
        })
}

pub(in crate::handlers) fn requires_buffered_response_stream(
    provider_rules: &[TransformRuleConfig],
    global_rules: &[TransformRuleConfig],
    auth_rules: &[TransformRuleConfig],
    model: &str,
    downstream: DownstreamProtocol,
) -> bool {
    provider_rules
        .iter()
        .chain(global_rules.iter())
        .chain(auth_rules.iter())
        .filter(|rule| rule.enabled && rule.phase == Phase::Response)
        .filter(|rule| match &rule.models {
            None => true,
            Some(patterns) => patterns
                .iter()
                .any(|pattern| model_glob_match(pattern, model)),
        })
        .any(|rule| {
            rule.transform == "image_markdown_to_output"
                && !matches!(downstream, DownstreamProtocol::Responses)
        })
}

pub(in crate::handlers) fn model_glob_match(pattern: &str, model: &str) -> bool {
    crate::glob::case_sensitive_glob_match(pattern, model)
}

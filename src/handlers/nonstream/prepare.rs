use super::exchange::upstream_route;
use super::*;

impl NonstreamRun<'_> {
    /// PIPE-1 request phase of one try: protocol shaping, request transforms,
    /// and upstream encoding. Returns the try with its encoded body; `Err`
    /// ends the request.
    pub(super) async fn prepare_call(
        &mut self,
        attempt: &MonoizeAttempt,
        slot: TrySlot,
        mut req: urp::UrpRequest,
    ) -> AppResult<(AttemptCall, Value)> {
        let auth = self.inputs.auth;
        let downstream = self.inputs.downstream;
        shape_request_for_attempt(&mut req, attempt, auth, downstream);
        let global_transforms = self.apply_request_transforms(attempt, &mut req).await?;
        strip_monoize_context(&mut req);
        let capture_transform_chain = crate::request_capture::build_transform_chain(
            &attempt.provider_transforms,
            &global_transforms,
            &auth.transforms,
            &self.transform_match_model,
        );
        let upstream_body = match encode_request_for_provider(&mut req, attempt, downstream) {
            Ok(body) => body,
            Err(err) => return Err(self.fail_request(attempt, err).await),
        };
        let responses_echo = responses_request_echo_for_attempt(&req, attempt, downstream);
        let (transport, path) = upstream_route(&req, attempt.provider_type);
        let call = AttemptCall {
            slot,
            req,
            transport,
            path,
            responses_echo,
            global_transforms,
            capture_transform_chain,
            capture_upstream_request: None,
            forwarded_headers: crate::error::ForwardedHeaders::new(),
        };
        Ok((call, upstream_body))
    }

    /// Provider, global, then API-key request transforms. Returns the global
    /// rules read for this try so the response phase applies the same set.
    async fn apply_request_transforms(
        &mut self,
        attempt: &MonoizeAttempt,
        req: &mut urp::UrpRequest,
    ) -> AppResult<Vec<TransformRuleConfig>> {
        let state = self.inputs.state;
        let auth = self.inputs.auth;
        self.apply_request_rules(attempt, req, &attempt.provider_transforms)
            .await?;
        let global_transforms = state.monoize_runtime.read().await.global_transforms.clone();
        self.apply_request_rules(attempt, req, &global_transforms)
            .await?;
        self.apply_request_rules(attempt, req, &auth.transforms)
            .await?;
        Ok(global_transforms)
    }

    async fn apply_request_rules(
        &mut self,
        attempt: &MonoizeAttempt,
        req: &mut urp::UrpRequest,
        rules: &[TransformRuleConfig],
    ) -> AppResult<()> {
        let applied = apply_transform_rules_request(
            self.inputs.state,
            req,
            rules,
            &self.transform_match_model,
            Some(attempt.provider_type),
        )
        .await;
        match applied {
            Ok(()) => Ok(()),
            Err(err) => Err(self.fail_request(attempt, err).await),
        }
    }
}

/// Shaping that precedes every request-phase transform: Responses tool
/// promotion, provider-item retention, the cross-family nested-extra strip,
/// Monoize context, the upstream model, and mz2 envelope unwrapping.
fn shape_request_for_attempt(
    req: &mut urp::UrpRequest,
    attempt: &MonoizeAttempt,
    auth: &crate::auth::AuthResult,
    downstream: DownstreamProtocol,
) {
    if matches!(downstream, DownstreamProtocol::Responses)
        || attempt.provider_type == ProviderType::Gemini
    {
        promote_responses_additional_tools(req, attempt.provider_type);
    }
    if let Some(target_protocol) = provider_type_protocol(attempt.provider_type) {
        urp::retain_provider_items_for_protocol(&mut req.input, target_protocol);
        if target_protocol == urp::ProviderProtocol::Responses {
            urp::remove_downstream_only_reasoning_for_responses(&mut req.input);
        }
    }
    if attempt.strip_cross_protocol_nested_extra
        && !downstream.is_same_family(attempt.provider_type)
    {
        urp::strip_nested_extra_body(&mut req.input);
    }
    inject_monoize_context(auth, req);
    req.model = attempt.upstream_model.clone();
    // Unwrap mz2 reasoning envelopes BEFORE any request-phase transform
    // observes the request input. Per spec/urp-transform-system.spec.md
    // PIPE-1 step 6 and PIPE-1d, transforms must not see encrypted
    // reasoning replays still in `mz2.` envelope form, and they must not
    // be allowed to mutate the reasoning payload before envelope-bound
    // provider/model checks (PR4c.6) decide whether to keep or drop the
    // replayed reasoning node for this attempt.
    urp::filter_and_unwrap_reasoning_envelopes_for_upstream(
        &mut req.input,
        reasoning_envelope_provider_type(attempt.provider_type),
        &req.model,
        auth.reasoning_envelope_enabled,
    );
}

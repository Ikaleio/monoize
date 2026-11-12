use super::*;

pub(in crate::handlers) fn media_resource_scope(
    attempt: &MonoizeAttempt,
) -> Option<urp::MediaResource> {
    use sha2::{Digest, Sha256};
    let protocol = provider_type_protocol(attempt.provider_type)?;
    let mut hash = Sha256::new();
    hash.update(attempt.base_url.as_bytes());
    hash.update([0]);
    hash.update(attempt.api_key.as_bytes());
    Some(urp::MediaResource {
        protocol,
        provider_id: Some(attempt.provider_id.clone()),
        channel_id: Some(attempt.channel_id.clone()),
        credential_scope: Some(format!("{:x}", hash.finalize())),
    })
}

pub(in crate::handlers) fn bind_media_request_routes(
    request: &mut urp::UrpRequest,
    attempts: &mut Vec<MonoizeAttempt>,
) -> AppResult<()> {
    let refs = urp::media::resources(&request.input).map_err(|message| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_file_reference", message)
    })?;
    if refs.is_empty() {
        return Ok(());
    }
    attempts.retain(|attempt| {
        media_resource_scope(attempt).is_some_and(|scope| {
            refs.iter()
                .all(|reference| urp::media::resource_matches_scope(reference, &scope))
        })
    });
    let scopes: Vec<_> = attempts.iter().filter_map(media_resource_scope).collect();
    let scope = scopes.first().ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "incompatible_file_reference",
            "No route matches the file reference source. Supply file bytes or a public URL.",
        )
    })?;
    if scopes.iter().any(|other| other != scope) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "ambiguous_file_reference",
            "The file reference is not bound to one provider and credential scope. Supply file bytes or use an unambiguous source route.",
        ));
    }
    urp::media::bind_resources(&mut request.input, scope);
    Ok(())
}

pub(in crate::handlers) fn validate_media_request_route(
    request: &urp::UrpRequest,
    attempt: &MonoizeAttempt,
) -> AppResult<()> {
    let refs = urp::media::resources(&request.input).map_err(|message| {
        AppError::new(StatusCode::BAD_REQUEST, "invalid_file_reference", message)
    })?;
    if refs.is_empty() {
        return Ok(());
    }
    let valid = media_resource_scope(attempt).is_some_and(|scope| {
        refs.iter()
            .all(|reference| urp::media::resource_matches_scope(reference, &scope))
    });
    if !valid {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "incompatible_file_reference",
            "The file reference cannot be used with this provider or credential scope.",
        ));
    }
    Ok(())
}

pub(in crate::handlers) fn convert_assistant_images_to_markdown(resp: &mut urp::UrpResponse) {
    let mut pending_markdown = String::new();
    let mut last_assistant_text_idx: Option<usize> = None;

    for (i, node) in resp.output.iter().enumerate() {
        match node {
            urp::Node::Image {
                role: urp::OrdinaryRole::Assistant,
                source,
                metadata,
                ..
            } => {
                let md = match source {
                    ImageSource::Url { url, .. } => format!("\n\n![image]({url})"),
                    ImageSource::Base64 { media_type, data } => {
                        format!("\n\n![image](data:{media_type};base64,{data})")
                    }
                    ImageSource::FileId { .. } => String::new(),
                };
                if !md.is_empty()
                    && let Some(prompt) = metadata
                        .image_generation
                        .revised_prompt
                        .as_deref()
                        .filter(|prompt| !prompt.is_empty())
                {
                    pending_markdown.push_str("\n\n");
                    pending_markdown.push_str(prompt);
                }
                pending_markdown.push_str(&md);
            }
            urp::Node::Text {
                role: urp::OrdinaryRole::Assistant,
                ..
            } => {
                last_assistant_text_idx = Some(i);
            }
            _ => {}
        }
    }

    if pending_markdown.is_empty() {
        return;
    }

    if let Some(idx) = last_assistant_text_idx {
        if let urp::Node::Text { content, .. } = &mut resp.output[idx] {
            content.push_str(&pending_markdown);
        }
    } else {
        resp.output.push(urp::Node::Text {
            logprobs: None,
            signature: None,
            citations: Vec::new(),
            id: None,
            role: urp::OrdinaryRole::Assistant,
            content: pending_markdown,
            phase: None,
            extra_body: std::collections::HashMap::new(),
            cache_breakpoint: None,
        });
    }

    resp.output.retain(|node| {
        !matches!(
            node,
            urp::Node::Image {
                role: urp::OrdinaryRole::Assistant,
                source: ImageSource::Url { .. } | ImageSource::Base64 { .. },
                ..
            }
        )
    });
}

/// Outcome of one streaming upstream dispatch for an image-capable attempt.
pub(in crate::handlers) struct ImageCapableStreamCall {
    /// Upstream path the request was sent to.
    pub path: String,
    /// RCD-D6/RCD-D6a capture `upstream_request`: the RCD-D16 multipart
    /// object for a multipart edit, otherwise the JSON upstream body. `None`
    /// when no capture session is active.
    pub capture_upstream_request: Option<Value>,
    pub result: Result<reqwest::Response, upstream::UpstreamCallError>,
}

/// Dispatch one raw upstream call, honoring OIU-S7: an `openai_image`
/// attempt with user image input goes to `POST /v1/images/edits`.
/// Inline Base64 edits use multipart; edits with references use JSON.
/// Other attempts post JSON using the attempt's upstream stream mode.
/// FP6j-2: `upstream_body` is dropped before the upstream response is awaited
/// unless `capture_active` keeps it for the capture dump.
/// `Err` is returned only for request-encode
/// failures that no retry can fix; upstream transport failures stay inside
/// `result` so callers keep their existing retry classification.
pub(in crate::handlers) async fn call_streaming_image_capable_upstream(
    http: &reqwest::Client,
    attempt: &MonoizeAttempt,
    req_attempt: &urp::UrpRequest,
    upstream_body: Value,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
    capture_active: bool,
) -> AppResult<ImageCapableStreamCall> {
    let provider = build_channel_provider_config(attempt);
    let openai_image_edit = attempt.provider_type == ProviderType::OpenaiImage
        && urp::encode::openai_image::has_user_image_input(req_attempt);
    if openai_image_edit && !urp::encode::openai_image::edit_requires_json(req_attempt) {
        drop(upstream_body);
        let path = "/v1/images/edits".to_string();
        let fields = urp::encode::openai_image::multipart_fields(req_attempt, &req_attempt.model)
            .map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        })?;
        let capture_upstream_request = capture_active.then(|| {
            crate::request_capture::multipart_capture_object_from_upstream_fields(&fields)
        });
        let form = urp::encode::openai_image::form_from_fields(fields).map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        })?;
        let result = upstream::call_upstream_multipart_with_timeout_and_headers(
            http,
            &provider,
            &attempt.api_key,
            &path,
            form,
            timeout_ms,
            extra_headers,
        )
        .await;
        return Ok(ImageCapableStreamCall {
            path,
            capture_upstream_request,
            result,
        });
    }
    let path = if openai_image_edit {
        "/v1/images/edits".to_string()
    } else {
        upstream_path_for_model(
            attempt.provider_type,
            &req_attempt.model,
            req_attempt.stream != Some(false),
        )
    };
    let body = upstream::JsonBody::new(&upstream_body);
    let capture_upstream_request = capture_active.then_some(upstream_body);
    let result = upstream::call_upstream_raw_with_timeout_and_headers(
        http,
        &provider,
        &attempt.api_key,
        &path,
        body,
        timeout_ms,
        extra_headers,
    )
    .await;
    Ok(ImageCapableStreamCall {
        path,
        capture_upstream_request,
        result,
    })
}
#[cfg(test)]
mod media_resource_routing_tests {
    use super::*;

    fn attempt(provider: &str, channel: &str, api_key: &str) -> MonoizeAttempt {
        MonoizeAttempt {
            provider_id: provider.into(),
            provider_name: provider.into(),
            provider_type: ProviderType::Responses,
            channel_id: channel.into(),
            channel_name: channel.into(),
            base_url: "https://api.example.com/v1".into(),
            api_key: api_key.into(),
            logical_model: "test-model".into(),
            upstream_model: "upstream-model".into(),
            model_multiplier: Multiplier::ONE,
            server_tool_usage_classes: vec![],
            provider_transforms: vec![],
            passive_failure_count_threshold: 0,
            passive_cooldown_seconds: 0,
            passive_window_seconds: 0,
            passive_rate_limit_cooldown_seconds: 0,
            channel_max_retries: 1,
            channel_retry_interval_ms: 0,
            circuit_breaker_enabled: false,
            per_model_circuit_break: false,
            provider_attempt_limit: None,
            request_timeout_ms: 1000,
            extra_fields_whitelist: None,
            strip_cross_protocol_nested_extra: false,
            model_price: None,
            pricing_model_key: "test-model".into(),
            allow_free_when_unpriced: true,
            allow_free_when_missing_usage: true,
            billing_group_id: None,
            group_billing_ratio: Multiplier::ONE,
            affinity_key: None,
            affinity_key_hash: None,
            affinity_hit: None,
            affinity_target: None,
            affinity_enabled: false,
            affinity_idle_ttl_seconds: 0,
            affinity_failback_mode: crate::monoize_routing::AffinityFailbackMode::Sticky,
            affinity_failback_delay_seconds: 0,
            routing_config_revision: 0,
            proxy_url: None,
            extra_headers: None,
            session_affinity_auto: false,
            websocket_supported: None,
            chat_dialect: urp::encode::openai_chat::ChatDialect::Generic,
            client_session_id: None,
            derived_session_affinity: None,
            session_affinity_value: None,
            client_anthropic_headers: Default::default(),
            origin_key: None,
            origin_peer_channel_ids: vec![],
        }
    }

    fn request(content: Value) -> urp::UrpRequest {
        urp::decode::openai_responses::decode_request(&json!({"model":"test-model",
            "input":[{"role":"user","content":[content]}]}))
        .unwrap()
    }

    fn private_request() -> urp::UrpRequest {
        request(json!({"type":"input_file","file_id":"file-original"}))
    }

    #[test]
    fn unbound_file_ids_reject_multiple_credential_channel_or_provider_scopes() {
        let source = attempt("provider-A", "channel-A", "key-A");
        for alternative in [
            attempt("provider-A", "channel-A", "key-B"),
            attempt("provider-A", "channel-B", "key-A"),
            attempt("provider-B", "channel-A", "key-A"),
        ] {
            let mut req = private_request();
            let mut attempts = vec![source.clone(), alternative];
            let error = bind_media_request_routes(&mut req, &mut attempts).unwrap_err();
            assert_eq!(error.code, "ambiguous_file_reference");
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            let refs = urp::media::resources(&req.input).unwrap();
            assert!(refs[0].credential_scope.is_none());
        }
    }

    #[test]
    fn a_bound_file_reference_keeps_only_its_eligible_candidate() {
        let allowed = attempt("provider-A", "channel-A", "key-A");
        let mut req = private_request();
        urp::media::bind_resources(&mut req.input, &media_resource_scope(&allowed).unwrap());
        let mut attempts = vec![
            attempt("provider-B", "channel-B", "key-B"),
            attempt("provider-A", "channel-B", "key-A"),
            allowed.clone(),
            attempt("provider-A", "channel-A", "wrong-key"),
        ];
        bind_media_request_routes(&mut req, &mut attempts).unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].provider_id, "provider-A");
        assert_eq!(attempts[0].channel_id, "channel-A");
        assert_eq!(attempts[0].api_key, "key-A");
        validate_media_request_route(&req, &allowed).unwrap();
        assert_eq!(
            urp::media::resources(&req.input).unwrap()[0]
                .provider_id
                .as_deref(),
            Some("provider-A")
        );
    }

    #[test]
    fn same_scope_retries_survive_binding_without_changing_the_source_reference() {
        let source = attempt("provider-A", "channel-A", "key-A");
        let mut retry = source.clone();
        retry.upstream_model = "compatible-retry-model".into();
        let mut attempts = vec![source, retry];
        let mut req = private_request();
        bind_media_request_routes(&mut req, &mut attempts).unwrap();
        assert_eq!(attempts.len(), 2);
        for attempt in &attempts {
            validate_media_request_route(&req, attempt).unwrap();
        }
        let wire =
            urp::encode::openai_responses::encode_request_checked(&req, "test-model").unwrap();
        assert_eq!(wire["input"][0]["content"][0]["file_id"], "file-original");
        assert!(!wire.to_string().contains("credential_scope"));
    }

    #[test]
    fn credential_or_endpoint_changes_cannot_reuse_an_already_bound_file() {
        let source = attempt("provider-A", "channel-A", "key-A");
        let mut req = private_request();
        bind_media_request_routes(&mut req, &mut vec![source.clone()]).unwrap();
        let mut different_key = source.clone();
        different_key.api_key = "key-B".into();
        let mut different_endpoint = source;
        different_endpoint.base_url = "https://other.example.com/v1".into();
        for changed in [different_key, different_endpoint] {
            let error = validate_media_request_route(&req, &changed).unwrap_err();
            assert_eq!(error.code, "incompatible_file_reference");
            let mut attempts = vec![changed];
            assert_eq!(
                bind_media_request_routes(&mut req.clone(), &mut attempts)
                    .unwrap_err()
                    .code,
                "incompatible_file_reference"
            );
            assert!(attempts.is_empty());
        }
    }

    #[test]
    fn public_urls_and_inline_bytes_leave_candidate_selection_unrestricted() {
        for content in [
            json!({"type":"input_file","file_url":"https://public.example.com/download"}),
            json!({"type":"input_file","file_data":"data:application/pdf;base64,JVBERi0="}),
        ] {
            let mut req = request(content);
            let mut attempts = vec![
                attempt("provider-A", "channel-A", "key-A"),
                attempt("provider-B", "channel-B", "key-B"),
            ];
            attempts[1].provider_type = ProviderType::Gemini;
            bind_media_request_routes(&mut req, &mut attempts).unwrap();
            assert_eq!(attempts.len(), 2);
            for attempt in &attempts {
                validate_media_request_route(&req, attempt).unwrap();
            }
            assert!(urp::media::resources(&req.input).unwrap().is_empty());
        }
    }

    fn png_sources(png: &str, data_url: &str) -> [(ProviderType, DownstreamProtocol, Value); 4] {
        [
            (
                ProviderType::ChatCompletion,
                DownstreamProtocol::ChatCompletions,
                json!({
                "model":"test-model", "messages":[{"role":"user","content":[
                    {"type":"image_url","image_url":{"url":data_url,"detail":"high"}},
                    {"type":"text","text":"Inspect image"},
                    {"type":"image_url","image_url":{"url":data_url,"detail":"high"}}
                ]}]}),
            ),
            (
                ProviderType::Responses,
                DownstreamProtocol::Responses,
                json!({
                "model":"test-model", "input":[{"role":"user","content":[
                    {"type":"input_image","image_url":data_url,"detail":"high"},
                    {"type":"input_text","text":"Inspect image"},
                    {"type":"input_image","image_url":data_url,"detail":"high"}
                ]}]}),
            ),
            (
                ProviderType::Messages,
                DownstreamProtocol::AnthropicMessages,
                json!({
                "model":"test-model", "max_tokens":64,"messages":[{"role":"user","content":[
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":png}},
                    {"type":"text","text":"Inspect image"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":png}}
                ]}]}),
            ),
            (
                ProviderType::Gemini,
                DownstreamProtocol::Responses,
                json!({
                "model":"test-model", "contents":[{"role":"user","parts":[
                    {"inlineData":{"mimeType":"image/png","data":png}},
                    {"text":"Inspect image"}, {"inlineData":{"mimeType":"image/png","data":png}}
                ]}]}),
            ),
        ]
    }

    fn decode(provider: ProviderType, native: &Value) -> urp::UrpRequest {
        match provider {
            ProviderType::ChatCompletion => urp::decode::openai_chat::decode_request(native),
            ProviderType::Responses => urp::decode::openai_responses::decode_request(native),
            ProviderType::Messages => urp::decode::anthropic::decode_request(native),
            ProviderType::Gemini => urp::decode::gemini::decode_request(native),
            _ => unreachable!(),
        }
        .unwrap()
    }

    fn assert_png_wire(
        source: ProviderType,
        target: ProviderType,
        stream: bool,
        wire: &Value,
        png: &str,
        data_url: &str,
    ) {
        let content = match target {
            ProviderType::ChatCompletion | ProviderType::Messages => {
                &wire["messages"][0]["content"]
            }
            ProviderType::Responses => &wire["input"][0]["content"],
            ProviderType::Gemini => &wire["contents"][0]["parts"],
            _ => unreachable!(),
        }
        .as_array()
        .unwrap();
        let native_images: Vec<_> = content
            .iter()
            .filter(|part| match target {
                ProviderType::ChatCompletion => part["type"] == "image_url",
                ProviderType::Responses => part["type"] == "input_image",
                ProviderType::Messages => part["type"] == "image",
                ProviderType::Gemini => part.get("inlineData").is_some(),
                _ => false,
            })
            .collect();
        assert_eq!(
            native_images.len(),
            2,
            "missing image {source:?} -> {target:?}, stream={stream}: {wire}"
        );
        for image in native_images {
            match target {
                ProviderType::ChatCompletion => {
                    assert_eq!(image["image_url"]["url"], data_url)
                }
                ProviderType::Responses => assert_eq!(image["image_url"], data_url),
                ProviderType::Messages => assert_eq!(
                    image["source"],
                    json!({"type":"base64","media_type":"image/png","data":png})
                ),
                ProviderType::Gemini => assert_eq!(
                    image["inlineData"],
                    json!({"mimeType":"image/png","data":png})
                ),
                _ => unreachable!(),
            }
        }
        assert_eq!(
            content.len(),
            3,
            "{source:?} -> {target:?}, stream={stream}: {wire}"
        );
        assert_eq!(content[1]["text"], "Inspect image");
        let decoded = decode(target, wire);
        let images: Vec<_> = decoded
            .input
            .iter()
            .filter_map(|node| match node {
                urp::Node::Image {
                    source: urp::ImageSource::Base64 { media_type, data },
                    ..
                } => Some((media_type.as_str(), data.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            images,
            vec![("image/png", png), ("image/png", png)],
            "{source:?} -> {target:?}, stream={stream}"
        );
    }

    #[test]
    fn cross_protocol_png_survives_production_request_encoding_matrix() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j6p8AAAAASUVORK5CYII=";
        let data_url = format!("data:image/png;base64,{png}");
        let sources = png_sources(png, &data_url);
        for (source, downstream, native) in sources {
            let canonical = decode(source, &native);
            for target in [
                ProviderType::ChatCompletion,
                ProviderType::Responses,
                ProviderType::Messages,
                ProviderType::Gemini,
            ] {
                for stream in [false, true] {
                    let mut candidate = attempt("image-provider", "image-channel", "image-key");
                    candidate.provider_type = target;
                    candidate.strip_cross_protocol_nested_extra = true;
                    let mut req = canonical.clone();
                    req.stream = Some(stream);
                    let mut candidates = vec![candidate];
                    bind_media_request_routes(&mut req, &mut candidates).unwrap();
                    let candidate = candidates.pop().unwrap();
                    let target_protocol = provider_type_protocol(target).unwrap();
                    urp::retain_provider_items_for_protocol(&mut req.input, target_protocol);
                    if target_protocol == urp::ProviderProtocol::Responses {
                        urp::remove_downstream_only_reasoning_for_responses(&mut req.input);
                    }
                    // Gemini has no downstream endpoint; its native fixture enters at the URP boundary.
                    let same_family = if source == ProviderType::Gemini {
                        target == source
                    } else {
                        downstream.is_same_family(target)
                    };
                    if candidate.strip_cross_protocol_nested_extra && !same_family {
                        urp::strip_nested_extra_body(&mut req.input);
                    }
                    req.model = candidate.upstream_model.clone();
                    let wire = encode_request_for_provider(&mut req, &candidate, downstream)
                        .unwrap_or_else(|err| {
                            panic!("{source:?} -> {target:?}, stream={stream}: {}", err.message)
                        });
                    assert_png_wire(source, target, stream, &wire, png, &data_url);
                }
            }
        }
    }

    #[test]
    fn completed_tool_media_without_local_call_survives_production_hygiene() {
        let png = "iVBORw0KGgo=";
        let image_url = format!("data:image/png;base64,{png}");
        for stream in [false, true] {
            for source_chat in [false, true] {
                let mut request = if source_chat {
                    urp::decode::openai_chat::decode_request(&json!({
                        "model":"test-model", "stream":stream, "messages":[
                            {"role":"tool", "tool_call_id":"call_history", "content":[
                                {"type":"text", "text":"screenshot"},
                                {"type":"image_url", "image_url":{"url":image_url}},
                                {"type":"file", "file":{"filename":"result.pdf", "file_data":"data:application/pdf;base64,JVBERi0xLjcK"}}
                            ]},
                            {"role":"user", "content":[{"type":"image_url", "image_url":{"url":image_url}}]}
                        ]
                    })).unwrap()
                } else {
                    urp::decode::openai_responses::decode_request(&json!({
                        "model":"test-model", "stream":stream, "input":[
                            {"type":"function_call_output", "call_id":"call_history", "output":[
                                {"type":"input_text", "text":"screenshot"},
                                {"type":"input_image", "image_url":image_url},
                                {"type":"input_file", "filename":"result.pdf", "file_data":"data:application/pdf;base64,JVBERi0xLjcK"}
                            ]},
                            {"role":"user", "content":[{"type":"input_image", "image_url":image_url}]}
                        ]
                    })).unwrap()
                };
                urp::retain_provider_items_for_protocol(
                    &mut request.input,
                    urp::ProviderProtocol::Responses,
                );
                urp::strip_nested_extra_body(&mut request.input);
                let mut candidates = vec![attempt("media", "channel", "key")];
                bind_media_request_routes(&mut request, &mut candidates).unwrap();
                let wire = encode_request_for_provider(
                    &mut request,
                    &candidates[0],
                    if source_chat {
                        DownstreamProtocol::ChatCompletions
                    } else {
                        DownstreamProtocol::Responses
                    },
                )
                .unwrap();
                assert_eq!(wire["input"].as_array().unwrap().len(), 2, "{wire}");
                assert_eq!(wire["input"][0]["type"], "function_call_output");
                assert_eq!(wire["input"][0]["call_id"], "call_history");
                assert_eq!(wire["input"][0]["output"][0]["text"], "screenshot");
                assert_eq!(wire["input"][0]["output"][1]["type"], "input_image");
                assert_eq!(wire["input"][0]["output"][1]["image_url"], image_url);
                assert_eq!(wire["input"][0]["output"][2]["filename"], "result.pdf");
                assert_eq!(wire["input"][1]["role"], "user");
                assert_eq!(wire["input"][1]["content"][0]["image_url"], image_url);
            }
        }
    }
}

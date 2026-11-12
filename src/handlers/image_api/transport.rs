use super::*;

pub(super) struct DispatchedImageAttempt {
    pub(super) call: ImageCapableStreamCall,
    pub(super) legacy: UrpRequest,
}

pub(super) async fn dispatch_image_attempt(
    execution: &ImageStreamExecution<'_>,
    prepared: &mut PreparedImageAttempt,
    http: &reqwest::Client,
    slot: &mut AttemptSlot,
    driver: &mut AttemptDriver,
    sink: &Option<&mut ImageStreamSink>,
) -> AppResult<DispatchedImageAttempt> {
    let mut negotiate_nonstream = false;
    loop {
        if negotiate_nonstream || prepared.request.stream == Some(false) {
            disable_image_streaming(&mut prepared.request);
        }
        let mut upstream_body = encode_request_for_provider(
            &mut prepared.request,
            &slot.attempt,
            DownstreamProtocol::Responses,
        )?;
        if prepared.request.stream == Some(false)
            && let Some(body) = upstream_body.as_object_mut()
        {
            body.remove("stream");
            body.remove("partial_images");
        }
        let legacy =
            typed_request_to_legacy(&prepared.request, execution.context.meta.max_multiplier)?;
        let extra_headers = attempt_extra_headers(&slot.attempt, &upstream_body);
        // Inline edits use multipart; reference edits use JSON.
        let call = call_streaming_image_capable_upstream(
            http,
            &slot.attempt,
            &prepared.request,
            upstream_body,
            slot.attempt
                .request_timeout_ms
                .saturating_mul(10)
                .max(600_000),
            &extra_headers,
            execution.context.capture.session.is_some(),
        )
        .await?;
        if !negotiate_nonstream
            && prepared.request.stream != Some(false)
            && !sink_frames_emitted(sink)
            && let Err(error) = &call.result
            && upstream_rejects_image_streaming(error)
        {
            let mask = execution
                .context
                .state
                .monoize_runtime
                .read()
                .await
                .mask_sensitive_info;
            let error = upstream_error_to_app(error.clone(), mask);
            execution
                .push_attempt(
                    slot,
                    &ImageAttemptCapture {
                        request: &prepared.request,
                        path: &call.path,
                        upstream_body: call.capture_upstream_request.as_ref(),
                        transform_chain: &prepared.transform_chain,
                    },
                    None,
                    Some(&error),
                )
                .await;
            driver.negotiate(slot, &error, mask);
            negotiate_nonstream = true;
            continue;
        }
        return Ok(DispatchedImageAttempt { call, legacy });
    }
}

fn disable_image_streaming(request: &mut urp::UrpRequest) {
    request.stream = Some(false);
    request.extra_body.remove("partial_images");
    if let Some(options) = &mut request.image_generation {
        options.partial_images = None;
    }
    for tool in request.tools.iter_mut().flatten() {
        if tool.tool_type == "image_generation" {
            tool.extra_body.remove("partial_images");
            if let Some(config) = tool.config.as_mut().and_then(Value::as_object_mut) {
                config.remove("partial_images");
            }
        }
    }
}

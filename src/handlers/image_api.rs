use super::*;
use std::collections::HashMap;

mod collection;
mod entrypoints;
mod execute;
mod output;
mod parsing;
mod request;
mod sink;
mod transport;

use collection::*;
pub use entrypoints::{create_image_edit, create_image_generation};
use execute::*;
use output::*;
use parsing::*;
use request::*;
use sink::*;
use transport::*;

struct ImageApiRequest {
    model: String,
    input: Vec<urp::Node>,
    options: ImageRequestOptions,
}

impl ImageApiRequest {
    fn into_urp(self, stream: bool) -> urp::UrpRequest {
        urp::UrpRequest {
            sampling: None,
            logprobs: None,
            context: Default::default(),
            instructions_format: None,
            model: self.model,
            input: self.input,
            stream: Some(stream),
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
            user: self.options.user,
            image_generation: Some(self.options.generation),
            extra_body: self.options.extra_body,
        }
    }
}

struct ImageRequestMeta {
    max_multiplier: Option<Multiplier>,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
    capture_raw_input: Arc<Value>,
    capture_protocol: crate::request_capture::CaptureDownstreamProtocol,
}

struct ImageExecutionMeta {
    max_multiplier: Option<Multiplier>,
    request_id: Option<String>,
    request_ip: Option<String>,
    client_session_id: Option<String>,
}

impl ImageRequestMeta {
    fn from_headers(
        headers: &HeaderMap,
        max_multiplier: Option<Multiplier>,
        capture_raw_input: Arc<Value>,
        capture_protocol: crate::request_capture::CaptureDownstreamProtocol,
    ) -> Self {
        Self {
            max_multiplier,
            request_id: extract_request_id(headers),
            request_ip: extract_client_ip(headers),
            client_session_id: extract_client_session_id(headers),
            capture_raw_input,
            capture_protocol,
        }
    }

    async fn capture(
        self,
        state: &AppState,
        auth: &crate::auth::AuthResult,
        stream: bool,
    ) -> (ImageExecutionMeta, RequestCaptureContext) {
        let session = state
            .request_capture
            .maybe_start_session(
                &state.monoize_runtime,
                auth,
                self.request_id.clone(),
                self.capture_protocol,
                stream,
            )
            .await;
        (
            ImageExecutionMeta {
                max_multiplier: self.max_multiplier,
                request_id: self.request_id,
                request_ip: self.request_ip,
                client_session_id: self.client_session_id,
            },
            RequestCaptureContext {
                raw_input: self.capture_raw_input,
                session,
            },
        )
    }
}

struct ImageExecutionContext<'a> {
    state: &'a AppState,
    auth: &'a crate::auth::AuthResult,
    meta: ImageExecutionMeta,
    capture: RequestCaptureContext,
    task_state: &'a AdmittedRequestTaskState,
}

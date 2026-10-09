//! `POST /v1/systemone` and `POST /v1/decisions` (`decision-api.spec.md`).

use super::*;
use crate::decision::{DecisionFormat, DecisionRequest, DecisionUsage};

pub async fn create_system_one(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    create_decision(state, headers, body, DecisionFormat::SystemOne).await
}

pub async fn create_decisions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    create_decision(state, headers, body, DecisionFormat::OpenaiDecisions).await
}

fn invalid_request(message: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message.into())
}

async fn create_decision(
    state: AppState,
    headers: HeaderMap,
    body: Value,
    downstream: DecisionFormat,
) -> AppResult<Response> {
    let auth = auth_tenant(&headers, &state).await?;
    let object = body
        .as_object()
        .ok_or_else(|| invalid_request("body must be object"))?;
    let mut logical_model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| invalid_request("missing model"))?
        .to_string();
    apply_configured_model_redirects_to_model(&state, &mut logical_model, &auth).await;
    ensure_model_allowed(&auth, &logical_model)?;
    downstream
        .validate_shape(object)
        .map_err(invalid_request)?;

    let max_multiplier = resolve_max_multiplier_for_embeddings(&body, &headers, &auth);
    let started_at = Instant::now();
    let routing_stub = build_embeddings_routing_stub(&logical_model, max_multiplier);
    let mut attempts = build_decision_attempts(&state, &routing_stub, &auth).await?;
    attach_client_session_id(&mut attempts, extract_client_session_id(&headers), None);
    let endpoint = DecisionForward::plan(downstream, object, &logical_model, &mut attempts)?;
    forward_json_attempts(
        JsonForward {
            state: &state,
            auth: &auth,
            headers: &headers,
            logical_model: &logical_model,
            started_at,
        },
        attempts,
        &endpoint,
    )
    .await
}

struct DecisionForward<'a> {
    downstream: DecisionFormat,
    logical_model: &'a str,
    /// Decoded downstream request. Present when a cross-format attempt exists.
    request: Option<DecisionRequest>,
    /// DR-R6: one upstream body per routed format, without `model`.
    bodies: Vec<(DecisionFormat, Map<String, Value>)>,
}

impl<'a> DecisionForward<'a> {
    /// DR-V7, DR-R5: computes the upstream body for every routed format and
    /// drops attempts whose format cannot carry the request.
    #[allow(clippy::result_large_err)]
    fn plan(
        downstream: DecisionFormat,
        body: &Map<String, Value>,
        logical_model: &'a str,
        attempts: &mut Vec<MonoizeAttempt>,
    ) -> AppResult<Self> {
        let routed = |format: DecisionFormat| {
            attempts
                .iter()
                .any(|attempt| attempt.provider_type.decision_format() == Some(format))
        };
        let mut request = None;
        let mut bodies = Vec::new();
        let mut first_error = None;
        for format in DecisionFormat::ALL.into_iter().filter(|format| routed(*format)) {
            if format == downstream {
                let mut same_format = body.clone();
                same_format.remove("max_multiplier");
                bodies.push((format, same_format));
                continue;
            }
            if request.is_none() {
                request = Some(downstream.decode_request(body).map_err(invalid_request)?);
            }
            let Some(decoded) = request.as_ref() else {
                continue;
            };
            match format.encode_request(decoded) {
                Ok(encoded) => bodies.push((format, encoded)),
                Err(message) => {
                    first_error.get_or_insert(message);
                }
            }
        }
        attempts.retain(|attempt| {
            bodies
                .iter()
                .any(|(format, _)| attempt.provider_type.decision_format() == Some(*format))
        });
        if attempts.is_empty()
            && let Some(message) = first_error
        {
            return Err(AppError::new(
                StatusCode::BAD_REQUEST,
                "unsupported_decision_input",
                message,
            ));
        }
        Ok(Self {
            downstream,
            logical_model,
            request,
            bodies,
        })
    }

    #[allow(clippy::result_large_err)]
    fn upstream_format(attempt: &MonoizeAttempt) -> AppResult<DecisionFormat> {
        attempt.provider_type.decision_format().ok_or_else(|| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "decision attempt routed to a non-decision channel",
            )
        })
    }
}

impl JsonForwardEndpoint for DecisionForward<'_> {
    fn upstream_request(&self, attempt: &MonoizeAttempt) -> AppResult<(&'static str, Value)> {
        let format = Self::upstream_format(attempt)?;
        let mut body = self
            .bodies
            .iter()
            .find(|(candidate, _)| *candidate == format)
            .map(|(_, body)| body.clone())
            .ok_or_else(|| {
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "missing upstream body for routed decision format",
                )
            })?;
        body.insert(
            "model".to_string(),
            Value::String(attempt.upstream_model.clone()),
        );
        Ok((format.upstream_path(), Value::Object(body)))
    }

    fn parse_usage(&self, upstream: &Value) -> Option<urp::Usage> {
        parse_usage_from_responses_object(upstream)
    }

    fn downstream_body(&self, attempt: &MonoizeAttempt, mut upstream: Value) -> AppResult<Value> {
        let upstream_format = Self::upstream_format(attempt)?;
        if upstream_format == self.downstream {
            if let Some(object) = upstream.as_object_mut() {
                object.insert(
                    "model".to_string(),
                    Value::String(self.logical_model.to_string()),
                );
            }
            return Ok(upstream);
        }
        // DR-S6: a malformed cross-format answer fails this attempt only.
        let decode_error = |message: String| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_decode_error",
                format!("cannot convert {} response: {message}", upstream_format.upstream_path()),
            )
        };
        let request = self
            .request
            .as_ref()
            .ok_or_else(|| decode_error("missing decoded request".to_string()))?;
        let answers = upstream_format
            .decode_answers(request, &upstream)
            .map_err(decode_error)?;
        self.downstream
            .encode_response(
                request,
                &answers,
                &DecisionUsage::from_body(&upstream),
                self.logical_model,
            )
            .map_err(decode_error)
    }
}

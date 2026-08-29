use super::*;
use crate::transforms::stream_split_sse_frames::DEFAULT_MAX_FRAME_LENGTH;
use crate::urp::ImageSource;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use xxhash_rust::xxh3::Xxh3;

#[allow(clippy::result_large_err)]
pub(super) fn decode_urp_request(
    protocol: DownstreamProtocol,
    known: Value,
    extra: Map<String, Value>,
) -> AppResult<urp::UrpRequest> {
    let merged = merge_known_and_extra(known, extra);
    let decoded = match protocol {
        DownstreamProtocol::Responses => urp::decode::openai_responses::decode_request(&merged),
        DownstreamProtocol::ChatCompletions => urp::decode::openai_chat::decode_request(&merged),
        DownstreamProtocol::AnthropicMessages => urp::decode::anthropic::decode_request(&merged),
    };
    decoded.map_err(|e| AppError::new(StatusCode::BAD_REQUEST, "invalid_request", e))
}

pub(super) fn merge_known_and_extra(known: Value, extra: Map<String, Value>) -> Value {
    let mut obj = known.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        obj.insert(k, v);
    }
    Value::Object(obj)
}

pub(super) fn resolve_max_multiplier(
    req: &urp::UrpRequest,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<Multiplier> {
    let ceiling = auth.max_multiplier;
    let requested =
        read_max_multiplier_from_extra(req).or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

pub(super) fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    crate::client_ip::canonical_client_ip_from_headers(headers).map(|address| address.to_string())
}

/// Reject the request if the API key has an IP whitelist and the client IP is not in it.
#[allow(clippy::result_large_err)]
pub(super) fn check_ip_whitelist(
    auth: &crate::auth::AuthResult,
    headers: &HeaderMap,
) -> AppResult<()> {
    if auth.ip_whitelist.is_empty() {
        return Ok(());
    }
    let client_ip = crate::client_ip::canonical_client_ip_from_headers(headers);
    let allowed = client_ip.is_some_and(|client_ip| {
        auth.ip_whitelist.iter().any(|entry| {
            entry
                .parse::<std::net::IpAddr>()
                .is_ok_and(|allowed| allowed == client_ip)
                || entry
                    .parse::<ipnet::IpNet>()
                    .is_ok_and(|network| network.contains(&client_ip))
        })
    });
    if !allowed {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "ip_not_allowed",
            "client IP is not in the API key whitelist",
        ));
    }
    Ok(())
}

pub(super) fn extract_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// CM-AFF-1a: read the client-supplied session-affinity value. Underscore and
/// hyphenated aliases are both accepted because some reverse proxies drop
/// header names that contain `_`. Values are sanitized per the shared affinity
/// sanitizer; an empty result means "absent".
pub(super) fn extract_client_session_id(headers: &HeaderMap) -> Option<String> {
    for name in [
        "session_id",
        "session-id",
        "x-session-id",
        "x-session-affinity",
    ] {
        if let Some(raw) = headers.get(name).and_then(|value| value.to_str().ok()) {
            let sanitized = crate::handlers::routing::sanitize_session_affinity(raw);
            if !sanitized.is_empty() {
                return Some(sanitized);
            }
        }
    }
    None
}

pub(super) fn read_max_multiplier_from_extra(req: &urp::UrpRequest) -> Option<Multiplier> {
    req.extra_body
        .get("max_multiplier")
        .and_then(Value::as_str)
        .and_then(parse_positive_multiplier)
}

pub(super) fn inject_monoize_context(auth: &crate::auth::AuthResult, req: &mut urp::UrpRequest) {
    if let Some(username) = &auth.username {
        req.extra_body
            .insert("__monoize_username".to_string(), json!(username.clone()));
    }
    if let Some(api_key_id) = &auth.api_key_id {
        req.extra_body.insert(
            "__monoize_api_key_id".to_string(),
            json!(api_key_id.clone()),
        );
    }
}

pub(super) fn strip_monoize_context(req: &mut urp::UrpRequest) {
    req.extra_body.remove("__monoize_username");
    req.extra_body.remove("__monoize_api_key_id");
}

pub(super) async fn apply_transform_rules_request(
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

pub(super) async fn apply_transform_rules_response(
    state: &AppState,
    resp: &mut urp::UrpResponse,
    rules: &[TransformRuleConfig],
    model: &str,
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
    })
}

pub(super) async fn transform_urp_stream(
    state: &AppState,
    mut rx: mpsc::Receiver<urp::UrpStreamEvent>,
    tx: mpsc::Sender<urp::UrpStreamEvent>,
    provider_rules: &[TransformRuleConfig],
    global_rules: &[TransformRuleConfig],
    auth_rules: &[TransformRuleConfig],
    model: &str,
    upstream_provider_type: Option<ProviderType>,
    reasoning_envelope: Option<(&str, &str)>,
) -> AppResult<()> {
    // The snapshot Arc is held for the whole stream so every event of one
    // request resolves against the same custom-transform set.
    let custom_snapshot = state.custom_transform_store.snapshot();
    let resolver = transforms::TransformResolver::new(
        state.transform_registry.as_ref(),
        custom_snapshot.as_ref(),
    );
    let mut provider_states = transforms::build_states_for_rules(provider_rules, resolver)
        .map_err(|e| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_init_failed",
                e.to_string(),
            )
        })?;
    let mut global_states =
        transforms::build_states_for_rules(global_rules, resolver).map_err(|e| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transform_init_failed",
                e.to_string(),
            )
        })?;
    let mut auth_states =
        transforms::build_states_for_rules(auth_rules, resolver).map_err(|e| {
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

    let mut reasoning_envelope_state = urp::ReasoningEnvelopeStreamState::default();
    while let Some(event) = rx.recv().await {
        // Fragment surfaces are assembled before envelope construction. This
        // keeps response transforms from observing raw fragments or a string
        // made by concatenating several independently wrapped envelopes.
        let enveloped_events = match reasoning_envelope {
            Some((provider_type, upstream_model)) => {
                reasoning_envelope_state.wrap_event(event, provider_type, upstream_model)
            }
            None => vec![event],
        };

        for event in enveloped_events {
            let provider_events = transforms::apply_stream_transforms(
                event,
                provider_rules,
                &mut provider_states,
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

            for provider_event in provider_events {
                let global_events = transforms::apply_stream_transforms(
                    provider_event,
                    global_rules,
                    &mut global_states,
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

                for global_event in global_events {
                    let auth_events = transforms::apply_stream_transforms(
                        global_event,
                        auth_rules,
                        &mut auth_states,
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

                    for auth_event in auth_events {
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

#[allow(clippy::result_large_err)]
pub(crate) fn typed_request_to_legacy(
    req: &urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
) -> AppResult<UrpRequest> {
    let encoded = urp::encode::openai_responses::encode_request(req, &req.model);
    let mut extra = Map::new();
    if let Some(limit) = max_multiplier {
        extra.insert(
            "max_multiplier".to_string(),
            Value::String(limit.to_string()),
        );
    }
    parse_urp_request(&encoded, extra)
}

fn affinity_value_from_json(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .as_i64()
                .map(|v| v.to_string())
                .or_else(|| value.as_u64().map(|v| v.to_string()))
        })
}

/// CM-AFF-1b: raw conversation identifier from the decoded request body.
/// Returns the identifier string itself so a header uuid and a body uuid match.
pub(super) fn stable_session_affinity_raw(req: &urp::UrpRequest) -> Option<String> {
    const SESSION_KEYS: &[&str] = &[
        "session_id",
        "session",
        "conversation_id",
        "conversation",
        "thread_id",
        "thread",
    ];
    for key in SESSION_KEYS {
        if let Some(value) = req.extra_body.get(*key).and_then(affinity_value_from_json) {
            return Some(value);
        }
    }
    if let Some(metadata) = req.extra_body.get("metadata").and_then(Value::as_object) {
        for key in SESSION_KEYS {
            if let Some(value) = metadata.get(*key).and_then(affinity_value_from_json) {
                return Some(value);
            }
        }
    }
    if let Some(value) = req
        .extra_body
        .get("user_id")
        .and_then(affinity_value_from_json)
    {
        return Some(value);
    }
    if let Some(metadata) = req.extra_body.get("metadata").and_then(Value::as_object)
        && let Some(value) = metadata.get("user_id").and_then(affinity_value_from_json)
    {
        return Some(value);
    }
    req.user
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn stable_affinity_field(req: &urp::UrpRequest) -> Option<String> {
    if let Some(previous_response_id) = req
        .extra_body
        .get("previous_response_id")
        .and_then(affinity_value_from_json)
    {
        return Some(format!("previous_response_id:{previous_response_id}"));
    }
    if let Some(user) = req.user.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        return Some(format!("user:{user}"));
    }
    const KEYS: &[&str] = &[
        "session_id",
        "session",
        "conversation_id",
        "conversation",
        "thread_id",
        "thread",
        "user_id",
        "user",
    ];
    for key in KEYS {
        if *key == "request_id" {
            continue;
        }
        if let Some(value) = req.extra_body.get(*key).and_then(affinity_value_from_json) {
            return Some(format!("{key}:{value}"));
        }
    }
    if let Some(metadata) = req.extra_body.get("metadata").and_then(Value::as_object) {
        for key in KEYS {
            if *key == "request_id" {
                continue;
            }
            if let Some(value) = metadata.get(*key).and_then(affinity_value_from_json) {
                return Some(format!("metadata.{key}:{value}"));
            }
        }
    }
    None
}

pub(crate) fn short_xxh3_hex(input: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(input.as_bytes()))
}

const AFFINITY_PREFIX_NODE_LIMIT: usize = 8;
const AFFINITY_PREFIX_BYTE_LIMIT: usize = 16 * 1024;

struct BoundedHashWriter {
    hasher: Xxh3,
    remaining: usize,
    limit_reached: bool,
}

impl BoundedHashWriter {
    fn new(limit: usize) -> Self {
        Self {
            hasher: Xxh3::new(),
            remaining: limit,
            limit_reached: false,
        }
    }

    fn digest(&self) -> u64 {
        self.hasher.digest()
    }
}

impl Write for BoundedHashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let accepted = self.remaining.min(buf.len());
        if accepted > 0 {
            self.hasher.update(&buf[..accepted]);
            self.remaining -= accepted;
        }
        if accepted < buf.len() {
            self.limit_reached = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "affinity prefix byte limit reached",
            ));
        }
        Ok(accepted)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// AFF-5a mirror of `urp::Node` that borrows every field and replaces the
/// flattened `HashMap` extras with a sorted `BTreeMap`. `HashMap` iteration
/// order is randomized per instance, so serializing `Node` directly would
/// hash identical requests to different affinity keys.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CanonicalAffinityNode<'a> {
    Text {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        content: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: &'a Option<String>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a ImageSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Audio {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a urp::AudioSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    File {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a urp::FileSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Refusal {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        content: &'a str,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: &'a Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: &'a Option<String>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        tool_type: urp::ToolCallType,
        call_id: &'a str,
        name: &'a str,
        arguments: &'a str,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ProviderItem {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        origin_protocol: urp::ProviderProtocol,
        role: urp::OrdinaryRole,
        item_type: &'a str,
        body: &'a Value,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        tool_type: urp::ToolCallType,
        call_id: &'a str,
        is_error: bool,
        content: Vec<CanonicalAffinityToolResultContent<'a>>,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    NextDownstreamEnvelopeExtra {
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CanonicalAffinityToolResultContent<'a> {
    Text {
        text: &'a str,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Image {
        source: &'a ImageSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    File {
        source: &'a urp::FileSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    ProviderItem {
        origin_protocol: urp::ProviderProtocol,
        item_type: &'a str,
        body: &'a Value,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
}

fn sorted_extra(extra_body: &HashMap<String, Value>) -> BTreeMap<&String, &Value> {
    extra_body.iter().collect()
}

fn canonical_affinity_tool_result_content(
    content: &urp::ToolResultContent,
) -> CanonicalAffinityToolResultContent<'_> {
    match content {
        urp::ToolResultContent::Text { text, extra_body } => {
            CanonicalAffinityToolResultContent::Text {
                text,
                extra_body: sorted_extra(extra_body),
            }
        }
        urp::ToolResultContent::Image { source, extra_body } => {
            CanonicalAffinityToolResultContent::Image {
                source,
                extra_body: sorted_extra(extra_body),
            }
        }
        urp::ToolResultContent::File { source, extra_body } => {
            CanonicalAffinityToolResultContent::File {
                source,
                extra_body: sorted_extra(extra_body),
            }
        }
        urp::ToolResultContent::ProviderItem {
            origin_protocol,
            item_type,
            body,
            extra_body,
        } => CanonicalAffinityToolResultContent::ProviderItem {
            origin_protocol: *origin_protocol,
            item_type,
            body,
            extra_body: sorted_extra(extra_body),
        },
    }
}

fn canonical_affinity_node(node: &urp::Node) -> CanonicalAffinityNode<'_> {
    match node {
        urp::Node::Text {
            id,
            role,
            content,
            phase,
            extra_body,
        } => CanonicalAffinityNode::Text {
            id,
            role: *role,
            content,
            phase,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Image {
            id,
            role,
            source,
            extra_body,
        } => CanonicalAffinityNode::Image {
            id,
            role: *role,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Audio {
            id,
            role,
            source,
            extra_body,
        } => CanonicalAffinityNode::Audio {
            id,
            role: *role,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::File {
            id,
            role,
            source,
            extra_body,
        } => CanonicalAffinityNode::File {
            id,
            role: *role,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Refusal {
            id,
            content,
            extra_body,
        } => CanonicalAffinityNode::Refusal {
            id,
            content,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => CanonicalAffinityNode::Reasoning {
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::ToolCall {
            id,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
        } => CanonicalAffinityNode::ToolCall {
            id,
            tool_type: *tool_type,
            call_id,
            name,
            arguments,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::ProviderItem {
            id,
            origin_protocol,
            role,
            item_type,
            body,
            extra_body,
        } => CanonicalAffinityNode::ProviderItem {
            id,
            origin_protocol: *origin_protocol,
            role: *role,
            item_type,
            body,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::ToolResult {
            id,
            tool_type,
            call_id,
            is_error,
            content,
            extra_body,
        } => CanonicalAffinityNode::ToolResult {
            id,
            tool_type: *tool_type,
            call_id,
            is_error: *is_error,
            content: content
                .iter()
                .map(canonical_affinity_tool_result_content)
                .collect(),
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::NextDownstreamEnvelopeExtra { extra_body } => {
            CanonicalAffinityNode::NextDownstreamEnvelopeExtra {
                extra_body: sorted_extra(extra_body),
            }
        }
    }
}

fn affinity_prefix_hash(req: &urp::UrpRequest) -> String {
    let mut writer = BoundedHashWriter::new(AFFINITY_PREFIX_BYTE_LIMIT);
    let result = (|| -> std::io::Result<()> {
        writer.write_all(b"[")?;
        for (index, node) in req
            .input
            .iter()
            .take(AFFINITY_PREFIX_NODE_LIMIT)
            .enumerate()
        {
            if index > 0 {
                writer.write_all(b",")?;
            }
            if let Err(error) = serde_json::to_writer(&mut writer, &canonical_affinity_node(node)) {
                if writer.limit_reached {
                    return Ok(());
                }
                return Err(std::io::Error::other(error));
            }
        }
        writer.write_all(b"]")
    })();
    if result.is_err() && !writer.limit_reached {
        return short_xxh3_hex("");
    }
    format!("{:016x}", writer.digest())
}

pub(super) fn build_routing_stub(
    req: &urp::UrpRequest,
    max_multiplier: Option<Multiplier>,
) -> UrpRequest {
    UrpRequest {
        model: req.model.clone(),
        max_multiplier,
        server_tool_usage_classes: server_tool_usage_classes(req.tools.as_deref()),
        affinity_explicit: stable_affinity_field(req),
        affinity_prefix_hash: affinity_prefix_hash(req),
    }
}

pub(super) fn build_embeddings_routing_stub(
    model: &str,
    max_multiplier: Option<Multiplier>,
) -> UrpRequest {
    UrpRequest {
        model: model.to_string(),
        max_multiplier,
        server_tool_usage_classes: Vec::new(),
        affinity_explicit: None,
        affinity_prefix_hash: short_xxh3_hex(model),
    }
}

pub(super) fn server_tool_usage_classes(tools: Option<&[urp::ToolDefinition]>) -> Vec<String> {
    let Some(tools) = tools else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tool in tools {
        let Some(class) = server_tool_usage_class(tool.tool_type.as_str()) else {
            continue;
        };
        if !out.iter().any(|existing| existing == class) {
            out.push(class.to_string());
        }
    }
    out
}

fn server_tool_usage_class(tool_type: &str) -> Option<&'static str> {
    match tool_type {
        "web_search" | "web_search_preview" | "web_fetch" => Some("web_search"),
        "file_search" | "collections_search" | "attachment_search" => Some("file_search_tool_call"),
        "x_search" => Some("x_search"),
        "code_interpreter" => Some("code_interpreter_duration"),
        "code_execution" => Some("code_execution_duration"),
        _ => None,
    }
}

pub(super) fn is_valid_embeddings_input(input: &Value) -> bool {
    if input.as_str().is_some() {
        return true;
    }
    input
        .as_array()
        .is_some_and(|arr| arr.iter().all(|item| item.as_str().is_some()))
}

pub(super) fn read_max_multiplier_from_embeddings_body(body: &Value) -> Option<Multiplier> {
    body.as_object()
        .and_then(|obj| obj.get("max_multiplier"))
        .and_then(Value::as_str)
        .and_then(parse_positive_multiplier)
}

pub(super) fn resolve_max_multiplier_for_embeddings(
    body: &Value,
    headers: &HeaderMap,
    auth: &crate::auth::AuthResult,
) -> Option<Multiplier> {
    let ceiling = auth.max_multiplier;
    let requested = read_max_multiplier_from_embeddings_body(body)
        .or_else(|| parse_max_multiplier_header(headers));

    match (ceiling, requested) {
        (Some(c), Some(r)) => Some(r.min(c)),
        (Some(c), None) => Some(c),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    }
}

pub(super) fn effective_sse_max_frame_length(
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

pub(super) fn requires_buffered_response_stream(
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

pub(super) fn convert_assistant_images_to_markdown(resp: &mut urp::UrpResponse) {
    let mut pending_markdown = String::new();
    let mut last_assistant_text_idx: Option<usize> = None;

    for (i, node) in resp.output.iter().enumerate() {
        match node {
            urp::Node::Image {
                role: urp::OrdinaryRole::Assistant,
                source,
                ..
            } => {
                let md = match source {
                    ImageSource::Url { url, .. } => format!("\n\n![image]({url})"),
                    ImageSource::Base64 { media_type, data } => {
                        format!("\n\n![image](data:{media_type};base64,{data})")
                    }
                    ImageSource::FileId { .. } => String::new(),
                };
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
            id: None,
            role: urp::OrdinaryRole::Assistant,
            content: pending_markdown,
            phase: None,
            extra_body: std::collections::HashMap::new(),
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

pub(super) fn model_glob_match(pattern: &str, model: &str) -> bool {
    crate::glob::case_sensitive_glob_match(pattern, model)
}

/// Default upstream extra_body field whitelists per provider type.
///
/// Fields that the URP request decoder already extracts into typed struct
/// fields (model, stream, temperature, etc.) are NOT in extra_body at all;
/// these lists cover only the keys that remain in `UrpRequest.extra_body`
/// and are safe to forward to the given upstream API.
const EXTRA_WHITELIST_CHAT_COMPLETION: &[&str] = &[
    "audio",
    "frequency_penalty",
    "function_call",
    "functions",
    "logit_bias",
    "logprobs",
    "top_logprobs",
    "max_completion_tokens",
    "max_tokens",
    "metadata",
    "moderation",
    "n",
    "presence_penalty",
    "prompt_cache_options",
    "safety_identifier",
    "seed",
    "service_tier",
    "stop",
    "stream_options",
    "store",
    "web_search_options",
    "parallel_tool_calls",
    "debug",
    "image_config",
    "modalities",
    "cache_control",
    "top_k",
    "top_a",
    "min_p",
    "repetition_penalty",
    "prediction",
    "prompt_cache_key",
    "prompt_cache_retention",
    "route",
    "structured_outputs",
    "verbosity",
    // OpenRouter / third-party extension fields
    "provider",
    "plugins",
    "session_id",
    "stop_server_tools_when",
    "trace",
    "thinking",
    "include_reasoning",
    "user_id",
];

const EXTRA_WHITELIST_RESPONSES: &[&str] = &[
    "background",
    "context_management",
    "conversation",
    "include",
    "instructions",
    "metadata",
    "max_tool_calls",
    "moderation",
    "parallel_tool_calls",
    "previous_response_id",
    "prompt",
    "prompt_cache_key",
    "prompt_cache_options",
    "prompt_cache_retention",
    "safety_identifier",
    "service_tier",
    "store",
    "stream_options",
    "text",
    "top_logprobs",
    "truncation",
];

const EXTRA_WHITELIST_ANTHROPIC: &[&str] = &[
    "cache_control",
    "container",
    "max_tokens",
    "metadata",
    "output_config",
    "service_tier",
    "stop_sequences",
    "top_k",
    "inference_geo",
];

const EXTRA_WHITELIST_GEMINI: &[&str] = &[
    "generationConfig",
    "safetySettings",
    "cachedContent",
    "labels",
];

const EXTRA_WHITELIST_OPENAI_IMAGE: &[&str] = &[
    "size",
    "quality",
    "style",
    "response_format",
    "n",
    "background",
    "output_format",
    "output_compression",
    "moderation",
    "user",
    "partial_images",
    "input_fidelity",
];

fn default_extra_whitelist(provider_type: ProviderType) -> &'static [&'static str] {
    match provider_type {
        ProviderType::ChatCompletion => EXTRA_WHITELIST_CHAT_COMPLETION,
        ProviderType::Responses => EXTRA_WHITELIST_RESPONSES,
        ProviderType::Messages => EXTRA_WHITELIST_ANTHROPIC,
        ProviderType::Gemini => EXTRA_WHITELIST_GEMINI,
        ProviderType::OpenaiImage => EXTRA_WHITELIST_OPENAI_IMAGE,
        ProviderType::Group => &[],
        // Replicate model input schemas are model-specific; whitelist is
        // handled inside the encoder by routing fields into `input`.
        ProviderType::Replicate => &["*"],
    }
}

/// Outcome of one streaming upstream dispatch for an image-capable attempt.
pub(super) struct ImageCapableStreamCall {
    /// Upstream path the request was sent to.
    pub path: String,
    /// RCD-D6a/OIU-E5g multipart capture object when the request was sent as
    /// multipart; `None` means the JSON `upstream_body` is the wire request.
    pub capture_multipart_request: Option<Value>,
    pub result: Result<reqwest::Response, upstream::UpstreamCallError>,
}

/// Dispatch one streaming upstream call, honoring OIU-S7: an `openai_image`
/// attempt whose URP request contains user image input goes to
/// `POST /v1/images/edits` as `multipart/form-data` (with the `stream` text
/// field from OIU-E5f); every other attempt posts the JSON `upstream_body` to
/// the provider's streaming path. `Err` is returned only for request-encode
/// failures that no retry can fix; upstream transport failures stay inside
/// `result` so callers keep their existing retry classification.
#[allow(clippy::too_many_arguments)]
pub(super) async fn call_streaming_image_capable_upstream(
    http: &reqwest::Client,
    attempt: &MonoizeAttempt,
    req_attempt: &urp::UrpRequest,
    upstream_body: &Value,
    timeout_ms: u64,
    extra_headers: &[(String, String)],
    capture_active: bool,
) -> AppResult<ImageCapableStreamCall> {
    let provider = build_channel_provider_config(attempt);
    let openai_image_edit = attempt.provider_type == ProviderType::OpenaiImage
        && urp::encode::openai_image::has_user_image_input(req_attempt);
    if openai_image_edit {
        let path = "/v1/images/edits".to_string();
        let fields = urp::encode::openai_image::multipart_fields(req_attempt, &req_attempt.model)
            .map_err(|message| {
            AppError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
        })?;
        let capture_multipart_request = capture_active.then(|| {
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
            capture_multipart_request,
            result,
        });
    }
    let path = upstream_path_for_model(attempt.provider_type, &req_attempt.model, true);
    let result = upstream::call_upstream_raw_with_timeout_and_headers(
        http,
        &provider,
        &attempt.api_key,
        &path,
        upstream_body,
        timeout_ms,
        extra_headers,
    )
    .await;
    Ok(ImageCapableStreamCall {
        path,
        capture_multipart_request: None,
        result,
    })
}

/// Filter `req.extra_body` to only contain fields allowed by the upstream
/// provider type's whitelist, optionally extended by a provider-level override.
///
/// If `provider_override` contains `"*"`, all fields pass through unfiltered.
pub(super) fn filter_extra_body_for_provider(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
    provider_override: &Option<Vec<String>>,
) {
    if let Some(overrides) = provider_override {
        if overrides.iter().any(|s| s == "*") {
            return;
        }
    }

    let defaults = default_extra_whitelist(provider_type);
    if defaults.contains(&"*") {
        return;
    }

    let override_set: HashSet<&str> = provider_override
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    req.extra_body.retain(|k, _| {
        k.starts_with("_monoize_")
            || defaults.contains(&k.as_str())
            || override_set.contains(k.as_str())
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderNativeToolFamily {
    Responses,
    Messages,
}

const RESPONSES_NATIVE_TOOL_TYPES: &[&str] = &[
    "file_search",
    "code_interpreter",
    "web_search",
    "web_search_preview",
    "mcp",
    "namespace",
    "tool_search",
    "programmatic_tool_calling",
    "image_generation",
    "computer",
    "computer_use_preview",
    "local_shell",
    "shell",
    "apply_patch",
];

const MESSAGES_NATIVE_TOOL_PREFIXES: &[&str] = &[
    "computer_",
    "web_search_",
    "web_fetch_",
    "code_execution_",
    "tool_search_tool_",
    "bash_",
    "text_editor_",
    "memory_",
    "advisor_",
];

const MESSAGES_NATIVE_TOOL_TYPES: &[&str] = &[
    "mcp_toolset",
    "tool_search_tool_bm25",
    "tool_search_tool_regex",
];

const RESPONSES_CUSTOM_MESSAGES_BRIDGE_EXTRA_KEY: &str =
    "_monoize_responses_custom_messages_bridge";

fn tool_wire_name(tool: &urp::ToolDefinition) -> Option<&str> {
    match tool.tool_type.as_str() {
        "function" => tool
            .function
            .as_ref()
            .map(|function| function.name.as_str()),
        "custom" => tool.custom.as_ref().map(|custom| custom.name.as_str()),
        _ => tool.name.as_deref(),
    }
}

fn collect_additional_tool_leaves(value: &Value, output: &mut Vec<Value>) {
    let Some(object) = value.as_object() else {
        return;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("namespace") => {
            if let Some(tools) = object.get("tools").and_then(Value::as_array) {
                for tool in tools {
                    collect_additional_tool_leaves(tool, output);
                }
            }
        }
        Some("function" | "custom") => output.push(value.clone()),
        _ => {}
    }
}

fn responses_additional_tool_leaves(req: &urp::UrpRequest) -> Vec<Value> {
    let mut output = Vec::new();
    for node in &req.input {
        let urp::Node::ProviderItem {
            origin_protocol: urp::ProviderProtocol::Responses,
            item_type,
            body,
            ..
        } = node
        else {
            continue;
        };
        if item_type != "additional_tools" {
            continue;
        }
        if let Some(tools) = body.get("tools").and_then(Value::as_array) {
            for tool in tools {
                collect_additional_tool_leaves(tool, &mut output);
            }
        }
    }
    output
}

pub(super) fn responses_additional_tools_require_messages_buffering(req: &urp::UrpRequest) -> bool {
    responses_additional_tool_leaves(req)
        .iter()
        .filter_map(urp::decode::parse_tool_definition)
        .any(|tool| tool.tool_type == "custom" && !custom_tool_has_messages_input_schema(&tool))
}

fn custom_tool_has_messages_input_schema(tool: &urp::ToolDefinition) -> bool {
    tool.custom.as_ref().is_some_and(|custom| {
        custom.extra_body.contains_key("input_schema")
            || tool.extra_body.contains_key("input_schema")
    })
}

fn messages_custom_bridge_function(tool: urp::ToolDefinition) -> urp::ToolDefinition {
    let custom = tool
        .custom
        .expect("custom tool promotion requires a custom definition");
    urp::ToolDefinition {
        tool_type: "function".to_string(),
        name: None,
        description: None,
        function: Some(urp::FunctionDefinition {
            name: custom.name,
            description: custom.description,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"],
                "additionalProperties": false
            })),
            strict: None,
            extra_body: HashMap::new(),
        }),
        custom: None,
        extra_body: HashMap::from([(
            RESPONSES_CUSTOM_MESSAGES_BRIDGE_EXTRA_KEY.to_string(),
            Value::Bool(true),
        )]),
    }
}

fn messages_custom_bridge_names(req: &urp::UrpRequest) -> HashSet<String> {
    req.tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|tool| {
            tool.extra_body
                .get(RESPONSES_CUSTOM_MESSAGES_BRIDGE_EXTRA_KEY)
                .and_then(Value::as_bool)
                == Some(true)
        })
        .filter_map(tool_wire_name)
        .map(ToOwned::to_owned)
        .collect()
}

fn bridge_messages_custom_history(req: &mut urp::UrpRequest) {
    let names = messages_custom_bridge_names(req);
    if names.is_empty() {
        return;
    }

    let mut bridged_call_ids = HashSet::new();
    for node in &mut req.input {
        let urp::Node::ToolCall {
            tool_type,
            call_id,
            name,
            arguments,
            ..
        } = node
        else {
            continue;
        };
        if *tool_type == urp::ToolCallType::Custom && names.contains(name) {
            *tool_type = urp::ToolCallType::Function;
            *arguments = json!({ "input": arguments.clone() }).to_string();
            bridged_call_ids.insert(call_id.clone());
        }
    }
    for node in &mut req.input {
        let urp::Node::ToolResult {
            tool_type, call_id, ..
        } = node
        else {
            continue;
        };
        if *tool_type == urp::ToolCallType::Custom && bridged_call_ids.contains(call_id) {
            *tool_type = urp::ToolCallType::Function;
        }
    }

    let Some(urp::ToolChoice::Specific(Value::Object(selector))) = req.tool_choice.as_mut() else {
        return;
    };
    if selector.get("type").and_then(Value::as_str) != Some("custom") {
        return;
    }
    let Some(name) = selector_name(selector, "custom")
        .filter(|name| names.contains(*name))
        .map(ToOwned::to_owned)
    else {
        return;
    };
    *selector = serde_json::Map::from_iter([
        ("type".to_string(), Value::String("function".to_string())),
        ("function".to_string(), json!({ "name": name })),
    ]);
}

pub(super) fn promote_responses_additional_tools(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
) {
    if !matches!(
        provider_type,
        ProviderType::ChatCompletion | ProviderType::Messages
    ) {
        return;
    }

    let mut names: HashSet<String> = req
        .tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(tool_wire_name)
        .map(ToOwned::to_owned)
        .collect();
    let mut promoted = Vec::new();
    for raw in responses_additional_tool_leaves(req) {
        let Some(mut tool) = urp::decode::parse_tool_definition(&raw) else {
            continue;
        };
        if provider_type == ProviderType::Messages
            && tool.tool_type == "custom"
            && !custom_tool_has_messages_input_schema(&tool)
        {
            tool = messages_custom_bridge_function(tool);
        }
        let Some(name) = tool_wire_name(&tool).map(ToOwned::to_owned) else {
            continue;
        };
        if names.insert(name) {
            promoted.push(tool);
        }
    }
    if !promoted.is_empty() {
        req.tools.get_or_insert_with(Vec::new).extend(promoted);
    }
    if provider_type == ProviderType::Messages {
        bridge_messages_custom_history(req);
    }
}

pub(super) fn restore_messages_custom_tool_calls(
    request: &urp::UrpRequest,
    response: &mut urp::UrpResponse,
) {
    let names = messages_custom_bridge_names(request);
    if names.is_empty() {
        return;
    }
    for node in &mut response.output {
        let urp::Node::ToolCall {
            tool_type,
            name,
            arguments,
            ..
        } = node
        else {
            continue;
        };
        if *tool_type != urp::ToolCallType::Function || !names.contains(name) {
            continue;
        }
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(arguments) else {
            continue;
        };
        let Some(input) = object.get("input").and_then(Value::as_str) else {
            continue;
        };
        *tool_type = urp::ToolCallType::Custom;
        *arguments = input.to_string();
    }
}

fn provider_native_tool_family(tool_type: &str) -> Option<ProviderNativeToolFamily> {
    if RESPONSES_NATIVE_TOOL_TYPES.contains(&tool_type) {
        return Some(ProviderNativeToolFamily::Responses);
    }
    if MESSAGES_NATIVE_TOOL_TYPES.contains(&tool_type)
        || MESSAGES_NATIVE_TOOL_PREFIXES
            .iter()
            .any(|prefix| tool_type.starts_with(prefix))
        || has_versioned_messages_native_tool_suffix(tool_type)
    {
        return Some(ProviderNativeToolFamily::Messages);
    }
    None
}

fn has_versioned_messages_native_tool_suffix(tool_type: &str) -> bool {
    tool_type
        .rsplit_once('_')
        .map(|(_, suffix)| suffix.len() == 8 && suffix.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(false)
}

fn provider_supports_native_tool_family(
    provider_type: ProviderType,
    family: ProviderNativeToolFamily,
) -> bool {
    matches!(
        (provider_type, family),
        (ProviderType::Responses, ProviderNativeToolFamily::Responses)
            | (ProviderType::Messages, ProviderNativeToolFamily::Messages)
    )
}

fn provider_supports_tool_definition(
    tool: &urp::ToolDefinition,
    provider_type: ProviderType,
    downstream: DownstreamProtocol,
) -> bool {
    if tool.tool_type == "function" {
        return true;
    }

    if tool.tool_type == "custom" {
        return provider_supports_custom_tool(tool, provider_type);
    }

    if let Some(family) = provider_native_tool_family(&tool.tool_type) {
        return provider_supports_native_tool_family(provider_type, family);
    }

    downstream.is_same_family(provider_type)
        && matches!(
            provider_type,
            ProviderType::Responses | ProviderType::Messages
        )
}

fn provider_supports_custom_tool(tool: &urp::ToolDefinition, provider_type: ProviderType) -> bool {
    match provider_type {
        ProviderType::ChatCompletion | ProviderType::Responses => tool.custom.is_some(),
        ProviderType::Messages => tool.custom.as_ref().is_some_and(|custom| {
            custom.extra_body.contains_key("input_schema")
                || tool.extra_body.contains_key("input_schema")
        }),
        _ => false,
    }
}

fn selector_name<'a>(obj: &'a serde_json::Map<String, Value>, kind: &str) -> Option<&'a str> {
    obj.get(kind)
        .and_then(Value::as_object)
        .and_then(|nested| nested.get("name"))
        .and_then(Value::as_str)
        .or_else(|| obj.get("name").and_then(Value::as_str))
}

fn selector_matches_tool(
    selector: &serde_json::Map<String, Value>,
    tool: &urp::ToolDefinition,
) -> bool {
    match selector.get("type").and_then(Value::as_str) {
        Some("function") => {
            tool.tool_type == "function"
                && selector_name(selector, "function")
                    .zip(
                        tool.function
                            .as_ref()
                            .map(|function| function.name.as_str()),
                    )
                    .is_some_and(|(selected, available)| selected == available)
        }
        Some("custom") => {
            tool.tool_type == "custom"
                && selector_name(selector, "custom")
                    .zip(tool.custom.as_ref().map(|custom| custom.name.as_str()))
                    .is_some_and(|(selected, available)| selected == available)
        }
        Some("mcp") => {
            tool.tool_type == "mcp"
                && selector
                    .get("server_label")
                    .and_then(Value::as_str)
                    .zip(tool.extra_body.get("server_label").and_then(Value::as_str))
                    .is_some_and(|(selected, available)| selected == available)
        }
        Some("auto" | "required" | "any" | "none" | "allowed_tools") | None => false,
        Some(native_type) => tool.tool_type == native_type,
    }
}

fn allowed_tool_references_mut(choice: &mut urp::ToolChoice) -> Option<&mut Vec<Value>> {
    let urp::ToolChoice::Specific(Value::Object(obj)) = choice else {
        return None;
    };
    if obj.get("type").and_then(Value::as_str) != Some("allowed_tools") {
        return None;
    }
    if obj.contains_key("allowed_tools") {
        return obj
            .get_mut("allowed_tools")
            .and_then(Value::as_object_mut)
            .and_then(|allowed| allowed.get_mut("tools"))
            .and_then(Value::as_array_mut);
    }
    obj.get_mut("tools").and_then(Value::as_array_mut)
}

pub(super) fn filter_tools_for_provider(
    req: &mut urp::UrpRequest,
    provider_type: ProviderType,
    downstream: DownstreamProtocol,
) {
    let Some(tools) = req.tools.as_mut() else {
        if matches!(req.tool_choice, Some(urp::ToolChoice::Specific(_))) {
            req.tool_choice = None;
        }
        return;
    };

    tools.retain(|tool| provider_supports_tool_definition(tool, provider_type, downstream));
    if tools.is_empty() {
        req.tools = None;
        req.tool_choice = None;
        return;
    }

    let Some(choice) = req.tool_choice.as_mut() else {
        return;
    };
    let is_allowed_tools = matches!(
        choice,
        urp::ToolChoice::Specific(Value::Object(obj))
            if obj.get("type").and_then(Value::as_str) == Some("allowed_tools")
    );
    if is_allowed_tools {
        if !matches!(
            provider_type,
            ProviderType::ChatCompletion | ProviderType::Responses
        ) {
            req.tool_choice = None;
            return;
        }
        let Some(references) = allowed_tool_references_mut(choice) else {
            req.tool_choice = None;
            return;
        };
        let available = req.tools.as_deref().unwrap_or_default();
        references.retain(|reference| {
            reference.as_object().is_some_and(|selector| {
                available
                    .iter()
                    .any(|tool| selector_matches_tool(selector, tool))
            })
        });
        if references.is_empty() {
            req.tool_choice = None;
        }
        return;
    }

    if let urp::ToolChoice::Specific(Value::Object(selector)) = choice
        && !matches!(
            selector.get("type").and_then(Value::as_str),
            Some("auto" | "required" | "any" | "none") | None
        )
        && !req
            .tools
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|tool| selector_matches_tool(selector, tool))
    {
        req.tool_choice = None;
    }
}

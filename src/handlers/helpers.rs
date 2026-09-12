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
/// header names that contain `_`. OpenCode clients send `x-opencode-session`.
/// Values are sanitized per the shared affinity sanitizer; an empty result
/// means "absent".
pub(super) fn extract_client_session_id(headers: &HeaderMap) -> Option<String> {
    for name in [
        "session_id",
        "session-id",
        "x-session-id",
        "x-opencode-session",
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
    req.context = urp::RequestContext {
        username: auth.username.clone(),
        api_key_id: auth.api_key_id.clone(),
    };
}

pub(super) fn strip_monoize_context(req: &mut urp::UrpRequest) {
    req.context = Default::default();
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
    urp::integerize_tool_call_nodes(&mut resp.output);
    Ok(())
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

                    for mut auth_event in auth_events {
                        urp::integerize_tool_call_stream_event(&mut auth_event);
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
    let mut legacy = build_routing_stub(req, max_multiplier);
    legacy.messages_custom_tool_names = messages_custom_bridge_names(req);
    Ok(legacy)
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
        signature: &'a Option<Value>,
        citations: &'a [Value],
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
        metadata: &'a urp::MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a ImageSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    Audio {
        metadata: &'a urp::MediaMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: &'a Option<String>,
        role: urp::OrdinaryRole,
        source: &'a urp::AudioSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    File {
        metadata: &'a urp::MediaMetadata,
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
        metadata: CanonicalAffinityReasoningMetadata<'a>,
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
        namespace: &'a Option<String>,
        signature: &'a Option<Value>,
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
        signature: &'a Option<Value>,
        namespace: &'a Option<String>,
        name: &'a Option<String>,
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
struct CanonicalAffinityReasoningMetadata<'a> {
    redacted: bool,
    downstream_only: bool,
    chat_content: bool,
    summary_as_thinking: bool,
    item_id: &'a Option<String>,
    summary_parts: Option<CanonicalAffinityReasoningParts<'a>>,
    content_parts: Option<CanonicalAffinityReasoningParts<'a>>,
}

struct CanonicalAffinityReasoningParts<'a>(&'a [urp::ReasoningTextPart]);

impl Serialize for CanonicalAffinityReasoningParts<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        #[derive(Serialize)]
        struct Entry<'a> {
            byte_length: usize,
            #[serde(flatten)]
            extra_body: BTreeMap<&'a String, &'a Value>,
        }
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for part in self.0 {
            sequence.serialize_element(&Entry {
                byte_length: part.byte_length,
                extra_body: sorted_extra(&part.extra_body),
            })?;
        }
        sequence.end()
    }
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
        metadata: &'a urp::MediaMetadata,
        source: &'a ImageSource,
        #[serde(flatten)]
        extra_body: BTreeMap<&'a String, &'a Value>,
    },
    File {
        metadata: &'a urp::MediaMetadata,
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
        urp::ToolResultContent::Image {
            source,
            metadata,
            extra_body,
        } => CanonicalAffinityToolResultContent::Image {
            metadata,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::ToolResultContent::File {
            source,
            metadata,
            extra_body,
        } => CanonicalAffinityToolResultContent::File {
            metadata,
            source,
            extra_body: sorted_extra(extra_body),
        },
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
            signature,
            citations,
            id,
            role,
            content,
            phase,
            extra_body,
        } => CanonicalAffinityNode::Text {
            signature,
            citations,
            id,
            role: *role,
            content,
            phase,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Image {
            metadata,
            id,
            role,
            source,
            extra_body,
            ..
        } => CanonicalAffinityNode::Image {
            metadata,
            id,
            role: *role,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::Audio {
            metadata,
            id,
            role,
            source,
            extra_body,
            ..
        } => CanonicalAffinityNode::Audio {
            metadata,
            id,
            role: *role,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::File {
            metadata,
            id,
            role,
            source,
            extra_body,
            ..
        } => CanonicalAffinityNode::File {
            metadata,
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
            metadata,
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body,
        } => CanonicalAffinityNode::Reasoning {
            metadata: CanonicalAffinityReasoningMetadata {
                redacted: metadata.redacted,
                downstream_only: metadata.downstream_only,
                chat_content: metadata.chat_content,
                summary_as_thinking: metadata.summary_as_thinking,
                item_id: &metadata.item_id,
                summary_parts: metadata
                    .summary_parts
                    .as_deref()
                    .map(CanonicalAffinityReasoningParts),
                content_parts: metadata
                    .content_parts
                    .as_deref()
                    .map(CanonicalAffinityReasoningParts),
            },
            id,
            content,
            encrypted,
            summary,
            source,
            extra_body: sorted_extra(extra_body),
        },
        urp::Node::ToolCall {
            namespace,
            signature,
            id,
            tool_type,
            call_id,
            name,
            arguments,
            extra_body,
            ..
        } => CanonicalAffinityNode::ToolCall {
            namespace,
            signature,
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
            signature,
            namespace,
            name,
            id,
            tool_type,
            call_id,
            is_error,
            content,
            extra_body,
            ..
        } => CanonicalAffinityNode::ToolResult {
            signature,
            namespace,
            name,
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

#[cfg(test)]
mod canonical_field_regressions {
    use super::*;

    fn fingerprint(node: Value) -> String {
        let request: urp::UrpRequest =
            serde_json::from_value(json!({"model":"m","input":[node]})).unwrap();
        affinity_prefix_hash(&request)
    }

    #[test]
    fn affinity_hash_distinguishes_typed_signatures_namespaces_and_media() {
        for (base, field, changed) in [
            (
                json!({"type":"text","role":"assistant","content":"same"}),
                "signature",
                json!("signature"),
            ),
            (
                json!({"type":"text","role":"assistant","content":"same"}),
                "citations",
                json!([{"uri":"https://example.com"}]),
            ),
            (
                json!({"type":"tool_call","call_id":"c","name":"run","arguments":"{}"}),
                "namespace",
                json!("functions"),
            ),
            (
                json!({"type":"tool_call","call_id":"c","name":"run","arguments":"{}"}),
                "signature",
                json!("signature"),
            ),
            (
                json!({"type":"tool_result","call_id":"c","content":[]}),
                "namespace",
                json!("functions"),
            ),
            (
                json!({"type":"tool_result","call_id":"c","content":[]}),
                "name",
                json!("run"),
            ),
            (
                json!({"type":"reasoning","content":"same"}),
                "metadata",
                json!({"redacted":true}),
            ),
        ] {
            let mut other = base.clone();
            other[field] = changed;
            assert_ne!(fingerprint(base), fingerprint(other), "missing {field}");
        }
        for kind in ["image", "audio", "file"] {
            let base = json!({"type":kind,"role":"assistant","source":{"type":"base64","media_type":"image/png","data":"bytes"}});
            let mut changed = base.clone();
            changed["metadata"] = json!({"signature":"new","media_type":"image/png","transcript":"words","expires_at":20});
            assert_ne!(
                fingerprint(base),
                fingerprint(changed),
                "missing {kind} metadata"
            );
        }
    }

    #[test]
    fn affinity_reasoning_part_unknown_fields_have_stable_order() {
        let left: urp::Node = serde_json::from_value(json!({"type":"reasoning","content":"same","metadata":{"content_parts":[{"byte_length":4,"z":3,"a":1,"m":2}]}})).unwrap();
        let right: urp::Node = serde_json::from_value(json!({"type":"reasoning","content":"same","metadata":{"content_parts":[{"m":2,"a":1,"z":3,"byte_length":4}]}})).unwrap();
        assert_eq!(
            serde_json::to_string(&canonical_affinity_node(&left)).unwrap(),
            serde_json::to_string(&canonical_affinity_node(&right)).unwrap()
        );
    }

    #[test]
    fn gemini_native_tools_and_allowed_lists_survive_provider_filtering() {
        let mut request = urp::decode::gemini::decode_request(&json!({"contents":[],"tools":[
            {"computerUse":{"environment":"ENVIRONMENT_BROWSER"}}, {"googleSearch":{}},
            {"functionDeclarations":[{"name":"run","parametersJsonSchema":{"type":"object"}}]}],
            "toolConfig":{"functionCallingConfig":{"mode":"VALIDATED","allowedFunctionNames":["run"]}}})).unwrap();
        filter_tools_for_provider(
            &mut request,
            ProviderType::Gemini,
            DownstreamProtocol::Responses,
        );
        assert_eq!(request.tools.as_ref().unwrap().len(), 3);
        assert!(request.tool_choice.is_some());
        assert_eq!(
            urp::encode::gemini::encode_request(&request, "m")["toolConfig"]["functionCallingConfig"],
            json!({"mode":"VALIDATED","allowedFunctionNames":["run"]})
        );
        filter_tools_for_provider(
            &mut request,
            ProviderType::Responses,
            DownstreamProtocol::Responses,
        );
        assert_eq!(request.tools.as_ref().unwrap().len(), 1);
        assert_eq!(request.tools.as_ref().unwrap()[0].tool_type, "function");
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
        audio_output_format: req
            .extra_body
            .get("audio")
            .and_then(|v| v.get("format"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        model: req.model.clone(),
        max_multiplier,
        server_tool_usage_classes: server_tool_usage_classes(req.tools.as_deref()),
        messages_custom_tool_names: HashSet::new(),
        affinity_explicit: stable_affinity_field(req),
        affinity_prefix_hash: affinity_prefix_hash(req),
    }
}

pub(super) fn media_resource_scope(attempt: &MonoizeAttempt) -> Option<urp::MediaResource> {
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

pub(super) fn bind_media_request_routes(
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

pub(super) fn validate_media_request_route(
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

pub(super) fn build_embeddings_routing_stub(
    model: &str,
    max_multiplier: Option<Multiplier>,
) -> UrpRequest {
    UrpRequest {
        audio_output_format: None,
        model: model.to_string(),
        max_multiplier,
        server_tool_usage_classes: Vec::new(),
        messages_custom_tool_names: HashSet::new(),
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
            signature: None,
            citations: Vec::new(),
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

const TOOL_NAMESPACE_BRIDGE_KEY: &str = "_monoize_tool_namespace_bridge";

fn collect_additional_tool_leaves(value: &Value, output: &mut Vec<Value>) {
    let Some(object) = value.as_object() else {
        return;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("namespace") => {
            if let Some(tools) = object.get("tools").and_then(Value::as_array) {
                for tool in tools {
                    let start = output.len();
                    collect_additional_tool_leaves(tool, output);
                    for leaf in &mut output[start..] {
                        if let Some(namespace) = object.get("name").and_then(Value::as_str) {
                            leaf["namespace"] = json!(namespace);
                        }
                    }
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
        namespace: None,
        tools: None,
        origin_protocol: None,
        config: None,

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
        ProviderType::ChatCompletion | ProviderType::Messages | ProviderType::Gemini
    ) {
        return;
    }

    fn append_explicit_tool(tool: urp::ToolDefinition, output: &mut Vec<urp::ToolDefinition>) {
        if tool.tool_type == "namespace" {
            let namespace = tool.name;
            for mut child in tool.tools.unwrap_or_default() {
                if child.namespace.is_none() {
                    child.namespace = namespace.clone();
                }
                append_explicit_tool(child, output);
            }
        } else {
            output.push(tool);
        }
    }
    let had_tools = req.tools.is_some();
    let mut candidates = Vec::new();
    for tool in req.tools.take().unwrap_or_default() {
        append_explicit_tool(tool, &mut candidates);
    }
    candidates.extend(
        responses_additional_tool_leaves(req)
            .iter()
            .filter_map(urp::decode::parse_tool_definition),
    );
    let mut names: HashSet<String> = candidates
        .iter()
        .filter(|tool| tool.namespace.is_none())
        .filter_map(|tool| tool_wire_name(tool).map(ToOwned::to_owned))
        .collect();
    let mut identities = HashSet::new();
    let mut promoted = Vec::new();
    for mut tool in candidates {
        if !matches!(tool.tool_type.as_str(), "function" | "custom") {
            promoted.push(tool);
            continue;
        }
        let namespace = tool.namespace.take();
        let Some(name) = tool_wire_name(&tool).map(ToOwned::to_owned) else {
            promoted.push(tool);
            continue;
        };
        if !identities.insert((namespace.clone(), name.clone())) {
            continue;
        }
        if let Some(namespace) = namespace {
            let prefix: String = format!("{namespace}_{name}")
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .take(40)
                .collect();
            let mut index = promoted.len();
            let alias = loop {
                let candidate = format!("{prefix}_{index}");
                if names.insert(candidate.clone()) {
                    break candidate;
                }
                index += 1;
            };
            if let Some(function) = &mut tool.function {
                function.name = alias.clone();
            }
            if let Some(custom) = &mut tool.custom {
                custom.name = alias;
            }
            tool.extra_body.insert(
                TOOL_NAMESPACE_BRIDGE_KEY.to_string(),
                json!({"namespace": namespace, "name": name}),
            );
        }
        if provider_type == ProviderType::Messages
            && tool.tool_type == "custom"
            && !custom_tool_has_messages_input_schema(&tool)
        {
            let identity = tool.extra_body.get(TOOL_NAMESPACE_BRIDGE_KEY).cloned();
            tool = messages_custom_bridge_function(tool);
            if let Some(identity) = identity {
                tool.extra_body
                    .insert(TOOL_NAMESPACE_BRIDGE_KEY.to_string(), identity);
            }
        }
        promoted.push(tool);
    }
    req.tools = (had_tools || !promoted.is_empty()).then_some(promoted);
    let aliases = tool_namespace_aliases(req);
    let mut call_aliases = HashMap::new();
    for node in &mut req.input {
        if let urp::Node::ToolCall {
            call_id,
            name,
            namespace,
            ..
        } = node
            && let Some(current_namespace) = namespace.as_deref()
            && let Some((alias, _)) = aliases.iter().find(|(_, identity)| {
                identity.get("namespace").and_then(Value::as_str) == Some(current_namespace)
                    && identity.get("name").and_then(Value::as_str) == Some(name.as_str())
            })
        {
            call_aliases.insert(call_id.clone(), alias.clone());
            *name = alias.clone();
            *namespace = None;
        }
    }
    for node in &mut req.input {
        if let urp::Node::ToolResult {
            call_id,
            name,
            namespace,
            ..
        } = node
        {
            let alias = match (namespace.as_deref(), name.as_deref()) {
                (Some(namespace), Some(name)) => aliases.iter().find_map(|(alias, identity)| {
                    (identity.get("namespace").and_then(Value::as_str) == Some(namespace)
                        && identity.get("name").and_then(Value::as_str) == Some(name))
                    .then_some(alias)
                }),
                (None, Some(_)) => call_aliases.get(call_id),
                _ => None,
            };
            if let Some(alias) = alias {
                *name = Some(alias.clone());
                *namespace = None;
            }
        }
    }
    if let Some(urp::ToolChoice::Specific(choice)) = &mut req.tool_choice {
        bridge_namespace_selector(choice, &aliases);
    }
    if provider_type == ProviderType::Messages {
        bridge_messages_custom_history(req);
    }
}

pub(super) fn tool_namespace_aliases(req: &urp::UrpRequest) -> HashMap<String, Value> {
    req.tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|tool| {
            Some((
                tool_wire_name(tool)?.to_string(),
                tool.extra_body.get(TOOL_NAMESPACE_BRIDGE_KEY)?.clone(),
            ))
        })
        .collect()
}

fn bridge_namespace_selector(value: &mut Value, aliases: &HashMap<String, Value>) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if matches!(kind.as_str(), "function" | "custom") {
        let namespace = obj
            .get("namespace")
            .or_else(|| obj.get(&kind)?.get("namespace"));
        if let Some(namespace) = namespace {
            let name = selector_name(obj, &kind);
            if let Some((alias, _)) = aliases.iter().find(|(_, identity)| {
                identity.get("namespace") == Some(namespace)
                    && identity.get("name").and_then(Value::as_str) == name
            }) {
                obj.remove("namespace");
                obj.remove("name");
                obj.insert(kind, json!({"name": alias}));
            }
        }
    } else if kind == "allowed_tools" {
        let tools = if obj.contains_key("allowed_tools") {
            obj.get_mut("allowed_tools")
                .and_then(|wrapper| wrapper.get_mut("tools"))
        } else {
            obj.get_mut("tools")
        };
        if let Some(tools) = tools.and_then(Value::as_array_mut) {
            for tool in tools {
                bridge_namespace_selector(tool, aliases);
            }
        }
    }
}

fn restore_tool_namespace(
    name: &mut String,
    target_namespace: &mut Option<String>,
    aliases: &HashMap<String, Value>,
) {
    if let Some(identity) = aliases.get(name) {
        if let (Some(original), Some(namespace)) = (
            identity.get("name").and_then(Value::as_str),
            identity.get("namespace").and_then(Value::as_str),
        ) {
            *name = original.to_string();
            *target_namespace = Some(namespace.to_string());
        }
    }
}

pub(super) fn restore_tool_namespace_node(node: &mut urp::Node, aliases: &HashMap<String, Value>) {
    if let urp::Node::ToolCall {
        name, namespace, ..
    } = node
    {
        restore_tool_namespace(name, namespace, aliases);
    }
}

pub(super) fn restore_tool_namespace_event(
    event: &mut urp::UrpStreamEvent,
    aliases: &HashMap<String, Value>,
) {
    match event {
        urp::UrpStreamEvent::NodeStart {
            header: urp::NodeHeader::ToolCall {
                name, namespace, ..
            },
            ..
        } => {
            restore_tool_namespace(name, namespace, aliases);
        }
        urp::UrpStreamEvent::NodeDone { node, .. } => restore_tool_namespace_node(node, aliases),
        urp::UrpStreamEvent::ResponseDone { output, .. } => {
            for node in output {
                restore_tool_namespace_node(node, aliases);
            }
        }
        _ => {}
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

    if let Some(origin) = tool.origin_protocol {
        return matches!(
            (origin, provider_type),
            (urp::ProviderProtocol::Gemini, ProviderType::Gemini)
                | (urp::ProviderProtocol::Responses, ProviderType::Responses)
                | (urp::ProviderProtocol::Messages, ProviderType::Messages)
                | (
                    urp::ProviderProtocol::ChatCompletion,
                    ProviderType::ChatCompletion
                )
        );
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
    if let Some(namespace) = selector.get("namespace").and_then(Value::as_str) {
        if tool.tool_type != "namespace" || tool.name.as_deref() != Some(namespace) {
            return false;
        }
        let mut leaf_selector = selector.clone();
        leaf_selector.remove("namespace");
        return tool.tools.as_ref().is_some_and(|tools| {
            tools
                .iter()
                .any(|leaf| selector_matches_tool(&leaf_selector, leaf))
        });
    }
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
                    .zip(
                        tool.config
                            .as_ref()
                            .and_then(|config| config.get("server_label"))
                            .or_else(|| tool.extra_body.get("server_label"))
                            .and_then(Value::as_str),
                    )
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
            ProviderType::ChatCompletion | ProviderType::Responses | ProviderType::Gemini
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

#[cfg(test)]
#[path = "tool_namespace_tests.rs"]
mod tool_namespace_tests;

#[cfg(test)]
mod routing_media_context_tests {
    use super::*;

    fn assert_context(request: &urp::UrpRequest, audio_format: Option<&str>) {
        let before = serde_json::to_value(request).unwrap();
        assert!(
            urp::encode::openai_responses::encode_request_checked(request, &request.model).is_err()
        );
        let context = typed_request_to_legacy(request, Some("1.5".parse().unwrap())).unwrap();
        assert_eq!(context.model, request.model);
        assert_eq!(context.audio_output_format.as_deref(), audio_format);
        assert_eq!(context.max_multiplier.unwrap().to_string(), "1.5");
        assert_eq!(serde_json::to_value(request).unwrap(), before);
    }

    #[test]
    fn chat_audio_context_does_not_require_a_responses_media_carrier() {
        for stream in [false, true] {
            let request = urp::decode::openai_chat::decode_request(&json!({
                "model":"chat-audio", "stream":stream, "audio":{"format":"pcm16", "voice":"alloy"},
                "messages":[{"role":"user", "content":[
                    {"type":"input_audio", "input_audio":{"data":"YQ==", "format":"wav"}}
                ]}]
            }))
            .unwrap();
            assert_context(&request, Some("pcm16"));
        }
    }

    #[test]
    fn gemini_private_file_context_does_not_require_a_responses_media_carrier() {
        for stream in [false, true] {
            let mut request = urp::decode::gemini::decode_request(&json!({
                "model":"gemini-files", "contents":[{"role":"user", "parts":[
                    {"fileData":{"mimeType":"application/pdf", "fileUri":"https://generativelanguage.googleapis.com/v1beta/files/source-file"}}
                ]}]
            })).unwrap();
            request.stream = Some(stream);
            assert_context(&request, None);
        }
    }

    #[test]
    fn gemini_audio_context_does_not_require_a_responses_media_carrier() {
        for stream in [false, true] {
            let mut request = urp::decode::gemini::decode_request(&json!({
                "model":"gemini-audio", "contents":[{"role":"user", "parts":[
                    {"inlineData":{"mimeType":"audio/wav", "data":"YQ=="}}
                ]}]
            }))
            .unwrap();
            request.stream = Some(stream);
            assert_context(&request, None);
        }
    }
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
            client_session_id: None,
            derived_session_affinity: None,
            session_affinity_value: None,
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

    #[test]
    fn cross_protocol_png_survives_production_request_encoding_matrix() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j6p8AAAAASUVORK5CYII=";
        let data_url = format!("data:image/png;base64,{png}");
        let sources = [
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
        ];
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
                    let decoded = decode(target, &wire);
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

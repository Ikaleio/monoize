//! Canonical prompt-cache breakpoints (`spec/urp-v2-flat-structure.spec.md` section 4a).

use super::encode::openai_chat::ChatDialect;
use super::grouping::continues_message;
use super::{
    CHAT_MESSAGE_BOUNDARY_EXTRA_KEY, CHAT_MESSAGE_MARKER_EXTRA_KEY, Node, OrdinaryRole,
    ProviderProtocol, ToolDefinition, ToolResultContent, UrpRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

const CACHE_CONTROL_KEY: &str = "cache_control";
const PROMPT_CACHE_BREAKPOINT_KEY: &str = "prompt_cache_breakpoint";
const MESSAGES_BREAKPOINT_LIMIT: usize = 4;
const CACHE_CONTROL_TTLS: &[&str] = &["5m", "1h"];

/// One marker at the end of a reusable prompt prefix (CB-1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
    pub origin: ProviderProtocol,
    pub wire: CacheBreakpointWire,
    #[serde(default, skip_serializing_if = "CacheBreakpointOwner::is_carrier")]
    pub owner: CacheBreakpointOwner,
    /// Set only by an Anthropic `cache_*` rule (CB-1a). Serialization drops it, so a marker that
    /// passes through a JavaScript transform falls back to client-owned, the safe direction.
    #[serde(skip)]
    pub anthropic_rule: bool,
    /// A map with deterministic order, because affinity hashes serialize this value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_body: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheBreakpointWire {
    CacheControl,
    PromptCacheBreakpoint,
}

/// The native marker layer when it is not the carrier object (CB-1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheBreakpointOwner {
    #[default]
    Carrier,
    MessageEnvelope,
    ToolPayload,
}

impl CacheBreakpointOwner {
    fn is_carrier(&self) -> bool {
        *self == Self::Carrier
    }
}

impl CacheBreakpoint {
    /// The marker of the Anthropic `cache_*` transforms (`auto-cache-transforms.spec.md` DEF-12).
    pub fn anthropic(ttl: Option<&str>) -> Self {
        Self {
            ttl: ttl.map(str::to_string),
            origin: ProviderProtocol::Messages,
            wire: CacheBreakpointWire::CacheControl,
            owner: CacheBreakpointOwner::Carrier,
            anthropic_rule: true,
            extra_body: BTreeMap::new(),
        }
    }

    /// The marker of `cache_openai_tool_use` (`auto-cache-transforms.spec.md` ACOTU-12).
    pub fn openai_responses() -> Self {
        Self {
            ttl: None,
            origin: ProviderProtocol::Responses,
            wire: CacheBreakpointWire::PromptCacheBreakpoint,
            owner: CacheBreakpointOwner::Carrier,
            anthropic_rule: false,
            extra_body: BTreeMap::new(),
        }
    }

    fn from_wire(
        mut object: Map<String, Value>,
        origin: ProviderProtocol,
        wire: CacheBreakpointWire,
    ) -> Self {
        let (implied_key, implied_value) = wire.implied_member();
        if object.get(implied_key).and_then(Value::as_str) == Some(implied_value) {
            object.remove(implied_key);
        }
        let ttl = match object.get("ttl") {
            Some(Value::String(ttl)) => Some(ttl.clone()),
            _ => None,
        };
        if ttl.is_some() {
            object.remove("ttl");
        }
        Self {
            ttl,
            origin,
            wire,
            owner: CacheBreakpointOwner::Carrier,
            anthropic_rule: false,
            extra_body: object.into_iter().collect(),
        }
    }

    fn replay_member(&self) -> (&'static str, Value) {
        (
            self.wire.key(),
            self.to_wire(self.wire, self.ttl.as_deref(), true),
        )
    }

    /// The wire object of this marker under `wire`. Unknown members exist only on a replay (CB-9).
    fn to_wire(&self, wire: CacheBreakpointWire, ttl: Option<&str>, replay: bool) -> Value {
        let (implied_key, implied_value) = wire.implied_member();
        let mut object = Map::new();
        object.insert(implied_key.into(), Value::String(implied_value.into()));
        if let Some(ttl) = ttl {
            object.insert("ttl".into(), Value::String(ttl.into()));
        }
        if replay {
            object.extend(self.extra_body.clone());
        }
        Value::Object(object)
    }
}

impl CacheBreakpointWire {
    fn key(self) -> &'static str {
        match self {
            Self::CacheControl => CACHE_CONTROL_KEY,
            Self::PromptCacheBreakpoint => PROMPT_CACHE_BREAKPOINT_KEY,
        }
    }

    fn implied_member(self) -> (&'static str, &'static str) {
        match self {
            Self::CacheControl => ("type", "ephemeral"),
            Self::PromptCacheBreakpoint => ("mode", "explicit"),
        }
    }
}

/// CB-4: selection is by key presence, so a malformed `cache_control` does not fall back.
fn marker_from_values(
    cache_control: Option<Value>,
    breakpoint: Option<Value>,
    origin: ProviderProtocol,
) -> Option<CacheBreakpoint> {
    let (value, wire) = match (cache_control, breakpoint) {
        (Some(value), _) => (value, CacheBreakpointWire::CacheControl),
        (None, Some(value)) => (value, CacheBreakpointWire::PromptCacheBreakpoint),
        (None, None) => return None,
    };
    let Value::Object(object) = value else {
        return None;
    };
    Some(CacheBreakpoint::from_wire(object, origin, wire))
}

fn take_wire_marker(
    extra_body: &mut HashMap<String, Value>,
    origin: ProviderProtocol,
) -> Option<CacheBreakpoint> {
    let own_cache_control = extra_body.remove(CACHE_CONTROL_KEY);
    let own_breakpoint = extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY);
    let envelope = extra_body.remove(CHAT_MESSAGE_MARKER_EXTRA_KEY);
    marker_from_values(own_cache_control, own_breakpoint, origin).or_else(|| {
        let Value::Object(mut envelope) = envelope? else {
            return None;
        };
        let mut marker = marker_from_values(
            envelope.remove(CACHE_CONTROL_KEY),
            envelope.remove(PROMPT_CACHE_BREAKPOINT_KEY),
            origin,
        )?;
        marker.owner = CacheBreakpointOwner::MessageEnvelope;
        Some(marker)
    })
}

/// CB-3a: moves a Chat message-level marker from the envelope `extra_body` to the last carrier
/// among `nodes`, the nodes of that one message.
pub(crate) fn attach_chat_message_marker(
    nodes: &mut [Node],
    extra_body: &mut HashMap<String, Value>,
) {
    let cache_control = extra_body.remove(CACHE_CONTROL_KEY);
    let breakpoint = extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY);
    let (key, value) = match (cache_control, breakpoint) {
        (Some(value), _) => (CACHE_CONTROL_KEY, value),
        (None, Some(value)) => (PROMPT_CACHE_BREAKPOINT_KEY, value),
        (None, None) => return,
    };
    extra_body.insert(
        CHAT_MESSAGE_BOUNDARY_EXTRA_KEY.to_string(),
        Value::Bool(true),
    );
    let carrier = nodes.iter_mut().rev().find_map(|node| match node {
        Node::Text { extra_body, .. }
        | Node::Image { extra_body, .. }
        | Node::Audio { extra_body, .. }
        | Node::File { extra_body, .. }
        | Node::ToolCall { extra_body, .. } => Some(extra_body),
        Node::Reasoning { .. }
        | Node::Refusal { .. }
        | Node::ProviderItem { .. }
        | Node::ToolResult { .. }
        | Node::NextDownstreamEnvelopeExtra { .. } => None,
    });
    if let Some(carrier) = carrier {
        carrier.insert(
            CHAT_MESSAGE_MARKER_EXTRA_KEY.to_string(),
            Value::Object(Map::from_iter([(key.to_string(), value)])),
        );
    }
}

/// CB-3: moves native markers from the extras of every carrier to the typed field.
pub(crate) fn decode_request_markers(req: &mut UrpRequest, origin: ProviderProtocol) {
    for tool in req.tools.iter_mut().flatten() {
        decode_tool_marker(tool, origin);
    }
    for node in &mut req.input {
        let marker = match node {
            Node::Text { extra_body, .. }
            | Node::Image { extra_body, .. }
            | Node::Audio { extra_body, .. }
            | Node::File { extra_body, .. }
            | Node::ToolCall { extra_body, .. }
            | Node::ToolResult { extra_body, .. } => take_wire_marker(extra_body, origin),
            _ => None,
        };
        if let Some(slot) = node.cache_breakpoint_mut() {
            *slot = marker;
        }
        if let Node::ToolResult { content, .. } = node {
            for item in content {
                let marker = take_wire_marker(item.extra_body_mut(), origin);
                if let Some(slot) = item.cache_breakpoint_mut() {
                    *slot = marker;
                }
            }
        }
    }
}

/// CB-3 layer order: tool descriptor, payload, `config`. Every layer loses both keys.
fn decode_tool_marker(tool: &mut ToolDefinition, origin: ProviderProtocol) {
    let mut layers: Vec<(Option<Value>, Option<Value>, CacheBreakpointOwner)> = Vec::new();
    layers.push((
        tool.extra_body.remove(CACHE_CONTROL_KEY),
        tool.extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY),
        CacheBreakpointOwner::Carrier,
    ));
    if let Some(function) = &mut tool.function {
        layers.push((
            function.extra_body.remove(CACHE_CONTROL_KEY),
            function.extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY),
            CacheBreakpointOwner::ToolPayload,
        ));
    }
    if let Some(custom) = &mut tool.custom {
        layers.push((
            custom.extra_body.remove(CACHE_CONTROL_KEY),
            custom.extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY),
            CacheBreakpointOwner::ToolPayload,
        ));
    }
    if let Some(Value::Object(config)) = &mut tool.config {
        layers.push((
            config.remove(CACHE_CONTROL_KEY),
            config.remove(PROMPT_CACHE_BREAKPOINT_KEY),
            CacheBreakpointOwner::Carrier,
        ));
    }
    tool.cache_breakpoint = layers
        .into_iter()
        .find(|(cache_control, breakpoint, _)| cache_control.is_some() || breakpoint.is_some())
        .and_then(|(cache_control, breakpoint, owner)| {
            let mut marker = marker_from_values(cache_control, breakpoint, origin)?;
            marker.owner = owner;
            Some(marker)
        });
    for child in tool.tools.iter_mut().flatten() {
        decode_tool_marker(child, origin);
    }
}

/// The DEF-2 count of `auto-cache-transforms.spec.md`.
pub(crate) fn anthropic_breakpoint_count(req: &UrpRequest) -> usize {
    let tools = req.tools.as_deref().map_or(0, tool_breakpoint_count);
    tools + input_breakpoint_count(req) + native_provider_marker_count(req)
}

fn tool_breakpoint_count(tools: &[ToolDefinition]) -> usize {
    tools
        .iter()
        .map(|tool| {
            usize::from(tool.cache_breakpoint.is_some())
                + tool.tools.as_deref().map_or(0, tool_breakpoint_count)
        })
        .sum()
}

/// The DEF-7 count of `auto-cache-transforms.spec.md`.
pub(crate) fn input_breakpoint_count(req: &UrpRequest) -> usize {
    req.input
        .iter()
        .map(|node| {
            let own = usize::from(node.cache_breakpoint().is_some());
            let nested = match node {
                Node::ToolResult { content, .. } => content
                    .iter()
                    .filter(|item| item.cache_breakpoint().is_some())
                    .count(),
                _ => 0,
            };
            own + nested
        })
        .sum()
}

fn native_provider_marker_count(req: &UrpRequest) -> usize {
    let has_marker = |body: &Value| body.get(CACHE_CONTROL_KEY).is_some();
    req.input
        .iter()
        .map(|node| {
            match node {
            Node::ProviderItem { body, .. } => usize::from(has_marker(body)),
            Node::ToolResult { content, .. } => content
                .iter()
                .filter(|item| {
                    matches!(item, ToolResultContent::ProviderItem { body, .. } if has_marker(body))
                })
                .count(),
            _ => 0,
        }
        })
        .sum()
}

/// `auto-cache-transforms.spec.md` DEF-10.
pub(crate) fn supports_openai_explicit_breakpoints(model: &str) -> bool {
    model
        .split(['/', ':'])
        .filter_map(parse_gpt_version)
        .any(|(major, minor)| major > 5 || (major == 5 && minor >= 6))
}

fn parse_gpt_version(segment: &str) -> Option<(u64, u64)> {
    let version = segment.strip_prefix("gpt-")?;
    let major_len = version.bytes().take_while(u8::is_ascii_digit).count();
    if major_len == 0 {
        return None;
    }
    let major = version[..major_len].parse().ok()?;
    let suffix = &version[major_len..];
    if suffix.is_empty() || suffix.starts_with('-') {
        return Some((major, 0));
    }
    let minor_text = suffix.strip_prefix('.')?;
    let minor_len = minor_text.bytes().take_while(u8::is_ascii_digit).count();
    if minor_len == 0 {
        return None;
    }
    let minor = minor_text[..minor_len].parse().ok()?;
    Some((major, minor))
}

/// The upstream wire that one request encode targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheWireTarget {
    Messages,
    Chat(ChatDialect),
    Responses,
}

impl CacheWireTarget {
    fn protocol(self) -> ProviderProtocol {
        match self {
            Self::Messages => ProviderProtocol::Messages,
            Self::Chat(_) => ProviderProtocol::ChatCompletion,
            Self::Responses => ProviderProtocol::Responses,
        }
    }
}

/// The wire object that holds one marker slot.
#[derive(Debug, Clone, Copy)]
enum Carrier {
    Text { assistant: bool },
    Media { assistant: bool },
    ToolCall,
    ToolResult,
    ToolResultText,
    ToolResultMedia,
    Tool,
}

/// Visits every marker slot in render order: tools, then input nodes, with the nested content of
/// a tool result before its own marker (CB-11).
fn for_each_slot<F>(req: &mut UrpRequest, visit: &mut F)
where
    F: FnMut(Carrier, &mut Option<CacheBreakpoint>, &mut HashMap<String, Value>),
{
    if let Some(tools) = &mut req.tools {
        for_each_tool_slot(tools, visit);
    }
    let assistant = |role: &OrdinaryRole| *role == OrdinaryRole::Assistant;
    for node in &mut req.input {
        match node {
            Node::Text {
                role,
                cache_breakpoint,
                extra_body,
                ..
            } => visit(
                Carrier::Text {
                    assistant: assistant(role),
                },
                cache_breakpoint,
                extra_body,
            ),
            Node::Image {
                role,
                metadata,
                extra_body,
                ..
            }
            | Node::Audio {
                role,
                metadata,
                extra_body,
                ..
            }
            | Node::File {
                role,
                metadata,
                extra_body,
                ..
            } => visit(
                Carrier::Media {
                    assistant: assistant(role),
                },
                &mut metadata.cache_breakpoint,
                extra_body,
            ),
            Node::ToolCall {
                cache_breakpoint,
                extra_body,
                ..
            } => visit(Carrier::ToolCall, cache_breakpoint, extra_body),
            Node::ToolResult {
                content,
                cache_breakpoint,
                extra_body,
                ..
            } => {
                for item in content {
                    match item {
                        ToolResultContent::Text {
                            cache_breakpoint,
                            extra_body,
                            ..
                        } => visit(Carrier::ToolResultText, cache_breakpoint, extra_body),
                        ToolResultContent::Image {
                            metadata,
                            extra_body,
                            ..
                        }
                        | ToolResultContent::File {
                            metadata,
                            extra_body,
                            ..
                        } => visit(
                            Carrier::ToolResultMedia,
                            &mut metadata.cache_breakpoint,
                            extra_body,
                        ),
                        ToolResultContent::ProviderItem { .. } => {}
                    }
                }
                visit(Carrier::ToolResult, cache_breakpoint, extra_body);
            }
            Node::Refusal { .. }
            | Node::Reasoning { .. }
            | Node::ProviderItem { .. }
            | Node::NextDownstreamEnvelopeExtra { .. } => {}
        }
    }
}

fn for_each_tool_slot<F>(tools: &mut [ToolDefinition], visit: &mut F)
where
    F: FnMut(Carrier, &mut Option<CacheBreakpoint>, &mut HashMap<String, Value>),
{
    for tool in tools {
        visit(
            Carrier::Tool,
            &mut tool.cache_breakpoint,
            &mut tool.extra_body,
        );
        if let Some(children) = &mut tool.tools {
            for_each_tool_slot(children, visit);
        }
    }
}

/// CB-10: Anthropic rejects a marker on an empty text block.
fn drop_converted_empty_text_markers(req: &mut UrpRequest, target: ProviderProtocol) {
    let drop_converted = |slot: &mut Option<CacheBreakpoint>| {
        if slot.as_ref().is_some_and(|marker| marker.origin != target) {
            *slot = None;
        }
    };
    for node in &mut req.input {
        match node {
            Node::Text {
                content,
                cache_breakpoint,
                ..
            } if content.is_empty() => drop_converted(cache_breakpoint),
            Node::ToolResult { content, .. } => {
                for item in content {
                    if let ToolResultContent::Text {
                        text,
                        cache_breakpoint,
                        ..
                    } = item
                        && text.is_empty()
                    {
                        drop_converted(cache_breakpoint);
                    }
                }
            }
            _ => {}
        }
    }
}

/// CB-11: only a request with a converted marker is limited, so a native Messages request reaches
/// the upstream unchanged and the upstream reports its own limit.
fn enforce_messages_limit(req: &mut UrpRequest) {
    let mut total = 0_usize;
    let mut converted = false;
    for_each_slot(req, &mut |_, slot, _| {
        if let Some(marker) = slot {
            total += 1;
            converted |= marker.origin != ProviderProtocol::Messages;
        }
    });
    if !converted {
        return;
    }
    let reserved = native_provider_marker_count(req)
        + usize::from(req.extra_body.contains_key(CACHE_CONTROL_KEY));
    let mut excess = total.saturating_sub(MESSAGES_BREAKPOINT_LIMIT.saturating_sub(reserved));
    for_each_slot(req, &mut |_, slot, _| {
        if excess > 0 && slot.take().is_some() {
            excess -= 1;
        }
    });
}

/// CB-15: Chat and Responses have no marker on the tool-result envelope.
fn move_converted_tool_result_markers(req: &mut UrpRequest, target: CacheWireTarget) {
    let protocol = target.protocol();
    for node in &mut req.input {
        let Node::ToolResult {
            content,
            cache_breakpoint,
            ..
        } = node
        else {
            continue;
        };
        if cache_breakpoint
            .as_ref()
            .is_none_or(|marker| marker.origin == protocol)
        {
            continue;
        }
        let marker = cache_breakpoint.take();
        let eligible = content.iter_mut().rev().find(|item| match item {
            ToolResultContent::Text { .. } => true,
            ToolResultContent::Image { .. } | ToolResultContent::File { .. } => {
                target == CacheWireTarget::Responses
            }
            ToolResultContent::ProviderItem { .. } => false,
        });
        if let Some(slot) = eligible.and_then(ToolResultContent::cache_breakpoint_mut)
            && slot.is_none()
        {
            *slot = marker;
        }
    }
}

fn accepted_ttl<'a>(marker: &'a CacheBreakpoint, accepted: &[&str]) -> Option<&'a str> {
    marker
        .ttl
        .as_deref()
        .filter(|ttl| accepted.iter().any(|value| value == ttl))
}

/// CB-8, CB-10, CB-12, CB-13: whether a marker replays on this target.
fn replays(marker: &CacheBreakpoint, target: CacheWireTarget) -> bool {
    use CacheBreakpointWire::{CacheControl, PromptCacheBreakpoint};

    if marker.origin != target.protocol() {
        return false;
    }
    match target {
        CacheWireTarget::Messages => marker.wire == CacheControl,
        CacheWireTarget::Chat(ChatDialect::DeepSeek) => false,
        CacheWireTarget::Chat(ChatDialect::OpenAi) | CacheWireTarget::Responses => {
            marker.wire == PromptCacheBreakpoint
        }
        CacheWireTarget::Chat(_) => true,
    }
}

/// CB-8 through CB-14: the wire member for one marker, or `None` when the target omits it.
fn wire_member(
    marker: &CacheBreakpoint,
    carrier: Carrier,
    target: CacheWireTarget,
    explicit_model: bool,
) -> Option<(&'static str, Value)> {
    use CacheBreakpointWire::{CacheControl, PromptCacheBreakpoint};

    if replays(marker, target) {
        return Some(marker.replay_member());
    }
    let convert = |wire: CacheBreakpointWire, accepted: &[&str]| {
        (
            wire.key(),
            marker.to_wire(wire, accepted_ttl(marker, accepted), false),
        )
    };
    match target {
        CacheWireTarget::Messages => Some(convert(CacheControl, CACHE_CONTROL_TTLS)),
        // CB-12a already handled every Vercel carrier that can convert.
        CacheWireTarget::Chat(ChatDialect::DeepSeek | ChatDialect::Vercel) => None,
        CacheWireTarget::Chat(ChatDialect::OpenAi) => (explicit_model
            && chat_part_carrier(carrier))
        .then(|| convert(PromptCacheBreakpoint, &["30m"])),
        CacheWireTarget::Chat(_) => (chat_part_carrier(carrier)
            || matches!(carrier, Carrier::Tool))
        .then(|| convert(CacheControl, CACHE_CONTROL_TTLS)),
        CacheWireTarget::Responses => {
            let eligible = match carrier {
                Carrier::Text { assistant } | Carrier::Media { assistant } => !assistant,
                Carrier::ToolResultText | Carrier::ToolResultMedia => true,
                Carrier::ToolCall | Carrier::ToolResult | Carrier::Tool => false,
            };
            (explicit_model && eligible).then(|| convert(PromptCacheBreakpoint, &["30m", "12h"]))
        }
    }
}

/// CB-18: keys outside CB-3, on nodes that hold no typed marker.
fn drop_forbidden_native_keys(req: &mut UrpRequest, target: CacheWireTarget) {
    let forbidden: &[&str] = match target {
        CacheWireTarget::Messages => &[PROMPT_CACHE_BREAKPOINT_KEY],
        CacheWireTarget::Chat(ChatDialect::OpenAi) | CacheWireTarget::Responses => {
            &[CACHE_CONTROL_KEY]
        }
        CacheWireTarget::Chat(ChatDialect::DeepSeek) => {
            &[CACHE_CONTROL_KEY, PROMPT_CACHE_BREAKPOINT_KEY]
        }
        CacheWireTarget::Chat(_) => &[],
    };
    for node in &mut req.input {
        if let Node::Reasoning { extra_body, .. }
        | Node::Refusal { extra_body, .. }
        | Node::NextDownstreamEnvelopeExtra { extra_body } = node
        {
            for key in forbidden {
                extra_body.remove(*key);
            }
        }
    }
}

/// CB-8: restores a replayed message-level marker on the control node of its message.
fn replay_message_markers(req: &mut UrpRequest, target: CacheWireTarget) {
    let mut envelope = None;
    for index in 0..req.input.len() {
        if matches!(req.input[index], Node::NextDownstreamEnvelopeExtra { .. }) {
            envelope = Some(index);
            continue;
        }
        let Some(envelope) = envelope else {
            continue;
        };
        let Some(slot) = req.input[index].cache_breakpoint_mut() else {
            continue;
        };
        let member = slot
            .as_ref()
            .filter(|marker| {
                marker.owner == CacheBreakpointOwner::MessageEnvelope && replays(marker, target)
            })
            .map(CacheBreakpoint::replay_member);
        let Some((key, value)) = member else {
            continue;
        };
        *slot = None;
        if let Node::NextDownstreamEnvelopeExtra { extra_body } = &mut req.input[envelope] {
            extra_body.insert(key.to_string(), value);
        }
    }
}

/// CB-8: restores a replayed tool marker on the payload that held it.
fn replay_tool_payload_markers(tools: &mut [ToolDefinition], target: CacheWireTarget) {
    for tool in tools {
        let member = tool
            .cache_breakpoint
            .as_ref()
            .filter(|marker| {
                marker.owner == CacheBreakpointOwner::ToolPayload && replays(marker, target)
            })
            .map(CacheBreakpoint::replay_member);
        if let Some((key, value)) = member {
            let payload = match (&mut tool.function, &mut tool.custom) {
                (Some(function), _) => Some(&mut function.extra_body),
                (None, Some(custom)) => Some(&mut custom.extra_body),
                (None, None) => None,
            };
            if let Some(payload) = payload {
                payload.insert(key.to_string(), value);
                tool.cache_breakpoint = None;
            }
        }
        if let Some(children) = &mut tool.tools {
            replay_tool_payload_markers(children, target);
        }
    }
}

fn chat_part_carrier(carrier: Carrier) -> bool {
    matches!(
        carrier,
        Carrier::Text { .. } | Carrier::Media { .. } | Carrier::ToolResultText
    )
}

/// CB-12a: places converted markers on Vercel's documented message level. A marker ends its wire
/// message, because a marker lifted to a later message end would widen the cached prefix.
fn materialize_vercel_message_markers(req: &mut UrpRequest) {
    let target = CacheWireTarget::Chat(ChatDialect::Vercel);
    drop_converted_empty_text_markers(req, ProviderProtocol::ChatCompletion);
    let converted = |slot: &mut Option<CacheBreakpoint>| {
        let marker = slot.take_if(|marker| !replays(marker, target))?;
        let wire = CacheBreakpointWire::CacheControl;
        Some((
            wire.key(),
            marker.to_wire(wire, accepted_ttl(&marker, CACHE_CONTROL_TTLS), false),
        ))
    };
    let mut output: Vec<Node> = Vec::with_capacity(req.input.len());
    let mut message_start = 0;
    let mut split_next = false;
    for mut node in std::mem::take(&mut req.input) {
        let continues = output
            .last()
            .is_some_and(|previous| continues_message(previous, &node));
        if !continues {
            message_start = output.len();
        } else if split_next {
            output.push(Node::NextDownstreamEnvelopeExtra {
                extra_body: HashMap::from([(
                    CHAT_MESSAGE_BOUNDARY_EXTRA_KEY.to_string(),
                    Value::Bool(true),
                )]),
            });
            message_start = output.len();
        }
        split_next = false;
        if let Node::ToolResult {
            content,
            extra_body,
            ..
        } = &mut node
        {
            let last_text = content
                .iter()
                .rposition(|item| matches!(item, ToolResultContent::Text { .. }));
            for (index, item) in content.iter_mut().enumerate() {
                if let ToolResultContent::Text {
                    cache_breakpoint, ..
                } = item
                    && let Some((key, value)) = converted(cache_breakpoint)
                    && Some(index) == last_text
                {
                    extra_body.insert(key.to_string(), value);
                }
            }
            output.push(node);
            continue;
        }
        let member = match &mut node {
            Node::Text {
                cache_breakpoint, ..
            } => converted(cache_breakpoint),
            Node::Image { metadata, .. }
            | Node::Audio { metadata, .. }
            | Node::File { metadata, .. } => converted(&mut metadata.cache_breakpoint),
            _ => None,
        };
        output.push(node);
        if let Some((key, value)) = member {
            output.insert(
                message_start,
                Node::NextDownstreamEnvelopeExtra {
                    extra_body: HashMap::from([(key.to_string(), value)]),
                },
            );
            split_next = true;
        }
    }
    req.input = output;
}

/// Resolves every typed marker for one target and writes the result as a wire member of the
/// owning extras. `req` MUST be the encoder's own clone (CB-17); the adapters then treat the
/// member like any other block field, including their scalar-collapse and instruction checks.
pub(crate) fn materialize_wire_markers(
    req: &mut UrpRequest,
    target: CacheWireTarget,
    upstream_model: &str,
) {
    if target == CacheWireTarget::Messages {
        drop_converted_empty_text_markers(req, ProviderProtocol::Messages);
        enforce_messages_limit(req);
    } else {
        move_converted_tool_result_markers(req, target);
    }
    drop_forbidden_native_keys(req, target);
    replay_message_markers(req, target);
    if let Some(tools) = &mut req.tools {
        replay_tool_payload_markers(tools, target);
    }
    if target == CacheWireTarget::Chat(ChatDialect::Vercel) {
        materialize_vercel_message_markers(req);
    }
    let explicit_model = supports_openai_explicit_breakpoints(upstream_model);
    for_each_slot(req, &mut |carrier, slot, extra_body| {
        let Some(marker) = slot.take() else {
            return;
        };
        if let Some((key, value)) = wire_member(&marker, carrier, target, explicit_model) {
            extra_body.insert(key.to_string(), value);
        }
    });
}

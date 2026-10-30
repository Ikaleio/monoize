//! Canonical prompt-cache breakpoints (`spec/urp-v2-flat-structure.spec.md` section 4a).

use super::encode::openai_chat::ChatDialect;
use super::{Node, OrdinaryRole, ProviderProtocol, ToolDefinition, ToolResultContent, UrpRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};

const CACHE_CONTROL_KEY: &str = "cache_control";
const PROMPT_CACHE_BREAKPOINT_KEY: &str = "prompt_cache_breakpoint";
const MESSAGES_BREAKPOINT_LIMIT: usize = 4;

/// One marker at the end of a reusable prompt prefix (CB-1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
    pub origin: ProviderProtocol,
    pub wire: CacheBreakpointWire,
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

impl CacheBreakpoint {
    /// The marker of the Anthropic `cache_*` transforms (`auto-cache-transforms.spec.md` DEF-12).
    pub fn anthropic(ttl: Option<&str>) -> Self {
        Self {
            ttl: ttl.map(str::to_string),
            origin: ProviderProtocol::Messages,
            wire: CacheBreakpointWire::CacheControl,
            extra_body: BTreeMap::new(),
        }
    }

    /// The marker of `cache_openai_tool_use` (`auto-cache-transforms.spec.md` ACOTU-12).
    pub fn openai_responses() -> Self {
        Self {
            ttl: None,
            origin: ProviderProtocol::Responses,
            wire: CacheBreakpointWire::PromptCacheBreakpoint,
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
            extra_body: object.into_iter().collect(),
        }
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

/// CB-4: `cache_control` wins when one carrier holds both keys.
fn take_wire_marker(
    extra_body: &mut HashMap<String, Value>,
    origin: ProviderProtocol,
) -> Option<CacheBreakpoint> {
    let cache_control = extra_body.remove(CACHE_CONTROL_KEY);
    let breakpoint = extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY);
    marker_from_values(cache_control, breakpoint, origin)
}

fn marker_from_values(
    cache_control: Option<Value>,
    breakpoint: Option<Value>,
    origin: ProviderProtocol,
) -> Option<CacheBreakpoint> {
    if let Some(Value::Object(object)) = cache_control {
        return Some(CacheBreakpoint::from_wire(
            object,
            origin,
            CacheBreakpointWire::CacheControl,
        ));
    }
    let Some(Value::Object(object)) = breakpoint else {
        return None;
    };
    Some(CacheBreakpoint::from_wire(
        object,
        origin,
        CacheBreakpointWire::PromptCacheBreakpoint,
    ))
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

fn decode_tool_marker(tool: &mut ToolDefinition, origin: ProviderProtocol) {
    let mut cache_control = tool.extra_body.remove(CACHE_CONTROL_KEY);
    let mut breakpoint = tool.extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY);
    if let Some(custom) = &mut tool.custom {
        cache_control = cache_control.or(custom.extra_body.remove(CACHE_CONTROL_KEY));
        breakpoint = breakpoint.or(custom.extra_body.remove(PROMPT_CACHE_BREAKPOINT_KEY));
    }
    if let Some(Value::Object(config)) = &mut tool.config {
        cache_control = cache_control.or(config.remove(CACHE_CONTROL_KEY));
        breakpoint = breakpoint.or(config.remove(PROMPT_CACHE_BREAKPOINT_KEY));
    }
    tool.cache_breakpoint = marker_from_values(cache_control, breakpoint, origin);
    for child in tool.tools.iter_mut().flatten() {
        decode_tool_marker(child, origin);
    }
}

/// The DEF-2 count of `auto-cache-transforms.spec.md`.
pub(crate) fn anthropic_breakpoint_count(req: &UrpRequest) -> usize {
    let tools = req
        .tools
        .iter()
        .flatten()
        .filter(|tool| tool.cache_breakpoint.is_some())
        .count();
    tools + input_breakpoint_count(req) + native_provider_marker_count(req)
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
fn for_each_slot(
    req: &mut UrpRequest,
    visit: &mut impl FnMut(Carrier, &mut Option<CacheBreakpoint>, &mut HashMap<String, Value>),
) {
    for tool in req.tools.iter_mut().flatten() {
        visit(
            Carrier::Tool,
            &mut tool.cache_breakpoint,
            &mut tool.extra_body,
        );
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

/// CB-10: Anthropic rejects a marker on an empty text block.
fn drop_converted_empty_text_markers(req: &mut UrpRequest) {
    for node in &mut req.input {
        if let Node::Text {
            content,
            cache_breakpoint,
            ..
        } = node
            && content.is_empty()
            && cache_breakpoint
                .as_ref()
                .is_some_and(|marker| marker.origin != ProviderProtocol::Messages)
        {
            *cache_breakpoint = None;
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

/// CB-8 through CB-14: the wire member for one marker, or `None` when the target omits it.
fn wire_member(
    marker: &CacheBreakpoint,
    carrier: Carrier,
    target: CacheWireTarget,
    explicit_model: bool,
) -> Option<(&'static str, Value)> {
    use CacheBreakpointWire::{CacheControl, PromptCacheBreakpoint};

    let same_family = marker.origin == target.protocol();
    let replay = |wire: CacheBreakpointWire| {
        (
            wire.key(),
            marker.to_wire(wire, marker.ttl.as_deref(), true),
        )
    };
    let convert = |wire: CacheBreakpointWire, accepted: &[&str]| {
        (
            wire.key(),
            marker.to_wire(wire, accepted_ttl(marker, accepted), false),
        )
    };
    const CACHE_CONTROL_TTLS: &[&str] = &["5m", "1h"];
    match target {
        CacheWireTarget::Messages => {
            if same_family && marker.wire == CacheControl {
                return Some(replay(CacheControl));
            }
            Some(convert(CacheControl, CACHE_CONTROL_TTLS))
        }
        CacheWireTarget::Chat(ChatDialect::DeepSeek) => None,
        CacheWireTarget::Chat(ChatDialect::OpenAi) => {
            if same_family && marker.wire == PromptCacheBreakpoint {
                return Some(replay(PromptCacheBreakpoint));
            }
            (explicit_model && chat_part_carrier(carrier))
                .then(|| convert(PromptCacheBreakpoint, &["30m"]))
        }
        CacheWireTarget::Chat(_) => {
            if same_family {
                return Some(replay(marker.wire));
            }
            (chat_part_carrier(carrier) || matches!(carrier, Carrier::Tool))
                .then(|| convert(CacheControl, CACHE_CONTROL_TTLS))
        }
        CacheWireTarget::Responses => {
            if same_family && marker.wire == PromptCacheBreakpoint {
                return Some(replay(PromptCacheBreakpoint));
            }
            let eligible = match carrier {
                Carrier::Text { assistant } | Carrier::Media { assistant } => !assistant,
                Carrier::ToolResultText | Carrier::ToolResultMedia => true,
                Carrier::ToolCall | Carrier::ToolResult | Carrier::Tool => false,
            };
            (explicit_model && eligible).then(|| convert(PromptCacheBreakpoint, &["30m", "12h"]))
        }
    }
}

fn chat_part_carrier(carrier: Carrier) -> bool {
    matches!(
        carrier,
        Carrier::Text { .. } | Carrier::Media { .. } | Carrier::ToolResultText
    )
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
        drop_converted_empty_text_markers(req);
        enforce_messages_limit(req);
    } else {
        move_converted_tool_result_markers(req, target);
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

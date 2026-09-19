use super::{ProviderProtocol, ToolCallType};
use serde::Deserialize;
use std::collections::HashMap;

/// Trusted request state that stays outside serialized URP and provider payloads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestContext {
    pub username: Option<String>,
    pub api_key_id: Option<String>,
    pub response_history: Option<ResponseHistoryContext>,
    pub tool_transports: HashMap<String, ToolTransport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHistoryContext {
    pub id: String,
    pub scope: String,
    pub store: bool,
    pub previous_response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    pub namespace: Option<String>,
    pub name: String,
    pub tool_type: ToolCallType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTransport {
    pub protocol: ProviderProtocol,
    pub wire_type: ToolCallType,
    pub original: ToolIdentity,
}

pub(super) fn discard_wire_context<'de, D>(deserializer: D) -> Result<RequestContext, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Consuming the field prevents serde flatten from treating it as provider extras.
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(RequestContext::default())
}

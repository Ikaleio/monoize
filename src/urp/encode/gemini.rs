//! Gemini GenerateContent encoding (gemini-codec.spec.md, unified_responses_proxy.spec.md
//! PG4 through PG8). Each submodule holds one concern; this root re-exports the public API and
//! what the Gemini stream encoder shares.

mod generation;
mod media;
mod model;
mod parts;
mod request;
mod response;
mod signatures;
mod tools;
mod usage;

pub(crate) use parts::content_extra;
pub use request::{encode_request, encode_request_checked};
pub use response::{encode_response, encode_response_checked};
pub(crate) use response::{
    encode_response_node_parts, is_prompt_block_response, native_failure_finish, response_failure,
};
pub(crate) use signatures::has_gemini_signature_provenance;
pub use tools::{gemini_builtin_tool_key, is_gemini_mappable_tool};

use crate::error::StreamErrorDetails;
use axum::http::StatusCode;
use serde_json::{Map, Value, json};

/// DG5: the Gemini error object for the HTTP error `status`, shared by every non-stream and
/// stream error.
pub(crate) fn error_json(status: StatusCode, message: &str) -> Value {
    let code = status.as_u16();
    let canonical = match code {
        400 => "INVALID_ARGUMENT",
        401 => "UNAUTHENTICATED",
        403 => "PERMISSION_DENIED",
        404 => "NOT_FOUND",
        409 => "ABORTED",
        429 => "RESOURCE_EXHAUSTED",
        499 => "CANCELLED",
        501 => "UNIMPLEMENTED",
        503 => "UNAVAILABLE",
        504 => "DEADLINE_EXCEEDED",
        ..=499 => "INVALID_ARGUMENT",
        _ => "INTERNAL",
    };
    json!({"error": {"code": code, "message": message, "status": canonical}})
}

/// DG2a rule 3: the HTTP status of a Gemini error, shared by response outcomes and stream
/// error events. The explicit status, else 499 for a cancellation, else the DG5 status of a
/// Google canonical code, else the SE2a status. A credential failure, including a canonical
/// code that maps to 401 or 403, becomes 502 under RTA-8b.
pub(crate) fn error_status(details: StreamErrorDetails<'_>, cancelled: bool) -> StatusCode {
    let status = match details.explicit_status {
        Some(status) => status,
        None if cancelled => client_closed_request(),
        None => canonical_code_status(details.code).unwrap_or_else(|| details.status()),
    };
    if details.is_credential_failure() || matches!(status.as_u16(), 401 | 403 | 407) {
        StatusCode::BAD_GATEWAY
    } else {
        status
    }
}

fn canonical_code_status(code: Option<&str>) -> Option<StatusCode> {
    Some(match code? {
        "INVALID_ARGUMENT" | "FAILED_PRECONDITION" | "OUT_OF_RANGE" => StatusCode::BAD_REQUEST,
        "UNAUTHENTICATED" => StatusCode::UNAUTHORIZED,
        "PERMISSION_DENIED" => StatusCode::FORBIDDEN,
        "NOT_FOUND" => StatusCode::NOT_FOUND,
        "ABORTED" | "ALREADY_EXISTS" => StatusCode::CONFLICT,
        "RESOURCE_EXHAUSTED" => StatusCode::TOO_MANY_REQUESTS,
        "CANCELLED" => client_closed_request(),
        "INTERNAL" | "UNKNOWN" | "DATA_LOSS" => StatusCode::INTERNAL_SERVER_ERROR,
        "UNIMPLEMENTED" => StatusCode::NOT_IMPLEMENTED,
        "UNAVAILABLE" => StatusCode::SERVICE_UNAVAILABLE,
        "DEADLINE_EXCEEDED" => StatusCode::GATEWAY_TIMEOUT,
        _ => return None,
    })
}

/// 499, the status Google maps to `CANCELLED`; `http` has no constant for it.
fn client_closed_request() -> StatusCode {
    StatusCode::from_u16(499).expect("499 is a valid HTTP status code")
}

/// The object member `key` of `object`, replacing a non-object value with `{}`.
fn object_member<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let member = object
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()));
    if !member.is_object() {
        *member = Value::Object(Map::new());
    }
    member.as_object_mut().expect("object member")
}

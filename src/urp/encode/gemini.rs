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
pub(crate) use response::{encode_response_node_parts, is_prompt_block_response};
pub use signatures::retain_gemini_signature_provenance;
pub use tools::{gemini_builtin_tool_key, is_gemini_mappable_tool};

use serde_json::{Map, Value};

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

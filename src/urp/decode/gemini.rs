//! Gemini GenerateContent decoding (gemini-codec.spec.md, unified_responses_proxy.spec.md
//! PG4 through PG8). Each submodule holds one concern; this root re-exports what the Gemini
//! encoders and the stream decoder share.

mod carriers;
mod config;
mod parts;
mod proto_names;
mod request;
mod response;
mod schema;
mod tools;
mod usage;

pub(crate) use carriers::{
    CONTINUATION_ID_PREFIX, ITEM_CARRIER_ID_PREFIX, PART_SIGNATURE_ID_PREFIX, SIGNATURE_ID_PREFIX,
    continuation_carrier, continuation_token, is_signature_carrier, item_carrier_offset,
    item_carrier_part, project_response_node, text_bytes, text_carrier_owner,
};
pub(crate) use config::{
    RESPONSE_SCHEMA_KEYS, TYPED_GENERATION_KEYS, TYPED_THINKING_KEYS, is_typed_response_mime,
    strip_text_response_format,
};
pub(crate) use parts::{
    SYNTHETIC_CALL_ID_PREFIX, append_fragment, content_parts, decode_part, node_extra,
};
pub use request::decode_request;
pub use response::decode_response;
pub(crate) use response::{
    candidate_extra, native_finish_reason, prompt_block_reason, prompt_refusal, response_outcome,
    selected_candidate,
};
pub(crate) use schema::native_schema_types;
pub(crate) use usage::parse_usage;

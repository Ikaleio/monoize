use crate::urp::{UrpRequest, UrpResponse};
use serde_json::Value;

pub fn decode_request(value: &Value) -> Result<UrpRequest, String> {
    crate::urp::decode::openai_responses::decode_request(value)
}

pub fn decode_response(value: &Value) -> Result<UrpResponse, String> {
    crate::urp::decode::openai_responses::decode_response(value)
}

use crate::urp::{UrpRequest, UrpResponse};
use serde_json::Value;

pub fn encode_request(req: &UrpRequest, upstream_model: &str) -> Value {
    crate::urp::encode::openai_responses::encode_request(req, upstream_model)
}

pub fn encode_response(resp: &UrpResponse, logical_model: &str) -> Value {
    crate::urp::encode::openai_responses::encode_response(resp, logical_model)
}

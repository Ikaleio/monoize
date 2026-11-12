//! OpenAI Responses encoder: URP requests and responses to Responses wire objects.

mod image_generation;
mod input_items;
mod items;
mod media;
mod reasoning;
mod request;
mod response;
mod tool_items;
mod tools;
mod usage;

pub(crate) use image_generation::encode_image_generation_call_item;
pub(crate) use items::encode_provider_item_for_responses;
pub(crate) use reasoning::merge_reasoning_extra;
pub(crate) use request::response_request_echo;
pub use request::{encode_request, encode_request_checked};
pub use response::{encode_response, encode_response_checked};
pub(crate) use response::{prepare_response_nodes, synthesized_response_controls};
pub(crate) use tool_items::encode_tool_result_output;
pub(crate) use usage::encode_usage;

// `media.rs` and the glob-importing test modules resolve `Value` / `HashMap` through `super::*`.
use serde_json::Value;
#[cfg(test)]
use std::collections::HashMap;

pub(crate) mod cache_breakpoint;
pub mod citations;
mod context;
pub mod decode;
pub mod encode;
pub mod extra_keys;
mod foreign;
pub mod grouping;
mod ids;
pub mod image_generation;
pub mod logprobs;
pub mod media;
mod node;
pub mod outcome;
pub mod reasoning;
mod reasoning_envelope;
mod request;
mod response;
pub mod sampling;
mod stream;
pub mod stream_decode;
pub mod stream_encode;
pub mod stream_events;
pub mod stream_helpers;
pub mod tool_call_json;
pub mod tool_signature;
pub mod usage;

pub use cache_breakpoint::{CacheBreakpoint, CacheBreakpointOwner, CacheBreakpointWire};
pub use citations::Citation;
pub use context::{
    AnthropicClientHeaders, RequestContext, ResponseHistoryContext, ToolIdentity, ToolTransport,
};
pub use extra_keys::*;
pub use foreign::*;
pub use ids::*;
pub use image_generation::ImageGenerationOptions;
pub use logprobs::{LogprobConfig, TokenLogprob};
pub use node::*;
pub use outcome::{ResponseOutcome, ResponseStatus};
pub use reasoning_envelope::*;
pub use request::*;
pub use response::*;
pub use sampling::SamplingConfig;
pub use stream::*;
pub use tool_call_json::{
    integerize_json_floats, integerize_tool_call_arguments_json, tool_call_arguments_for_wire,
};
pub use usage::{InputDetails, ModalityBreakdown, OutputDetails, Usage, UsageIteration};

#[cfg(test)]
mod chat_feature_tests;
#[cfg(test)]
mod gemini_feature_tests;
#[cfg(test)]
mod media_transport_tests;
#[cfg(test)]
mod messages_feature_tests;
#[cfg(test)]
mod protocol_contract_tests;
#[cfg(test)]
mod responses_feature_tests;

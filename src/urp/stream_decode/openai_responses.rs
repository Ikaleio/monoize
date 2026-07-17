use crate::error::{AppError, AppResult};
use crate::handlers::routing::now_ts;
use crate::handlers::usage::{
    mark_stream_ttfb_if_needed, parse_usage_from_responses_object, record_stream_done_sentinel,
    record_stream_terminal_error, record_stream_terminal_event, record_stream_usage_if_present,
    record_visible_stream_event_delta,
};
use crate::handlers::{StreamRuntimeMetrics, StreamTerminalError, UrpRequest as HandlerUrpRequest};
#[cfg(test)]
use crate::urp::internal_legacy_bridge::nodes_to_items;
use crate::urp::internal_legacy_bridge::{Item, Part, Role};
use crate::urp::stream_helpers::{
    extract_reasoning_parts, extract_responses_message_phase, extract_responses_message_text,
};
use crate::urp::{
    FinishReason, Node, NodeDelta, NodeHeader, OrdinaryRole, ProviderProtocol,
    RESPONSES_IMAGE_GENERATION_CALL_EXTRA_KEY, RESPONSES_STREAM_START_SOURCE_EXTRA_KEY,
    ToolCallType, UrpStreamEvent, node_is_empty_text, nodes_semantically_match,
};
use axum::http::StatusCode;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

include!("openai_responses/image_helpers.inc.rs");
include!("openai_responses/stream_loop_part1.inc.rs");
include!("openai_responses/stream_loop_part2.inc.rs");
include!("openai_responses/event_map.inc.rs");
include!("openai_responses/state.inc.rs");
include!("openai_responses/output_events.inc.rs");
include!("openai_responses/completed.inc.rs");
include!("openai_responses/decode_helpers.inc.rs");
include!("openai_responses/tests.inc.rs");

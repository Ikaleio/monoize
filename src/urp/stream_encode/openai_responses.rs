use crate::error::AppResult;
use crate::handlers::routing::now_ts;
use crate::urp::stream_helpers::*;
use crate::urp::{self, ToolResultContent, UrpStreamEvent};
use axum::response::sse::Event;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tokio::sync::mpsc;

include!("openai_responses/state.inc.rs");
include!("openai_responses/synthetic.inc.rs");
include!("openai_responses/encode_loop_part1.inc.rs");
include!("openai_responses/encode_loop_part2.inc.rs");
include!("openai_responses/node_mapping.inc.rs");
include!("openai_responses/completion.inc.rs");
include!("openai_responses/terminal_events.inc.rs");
include!("openai_responses/media_helpers.inc.rs");
include!("openai_responses/tests.inc.rs");

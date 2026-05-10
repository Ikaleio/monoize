use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::response::Sse;
use axum::response::sse::Event;
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tower::ServiceExt;

type CapturedHeaders = Arc<Mutex<Vec<(String, String)>>>;
type CapturedBodies = Arc<Mutex<Vec<(String, Value)>>>;

struct TestContext {
    router: axum::Router,
    auth_header: String,
    state: monoize::app::AppState,
    captured_headers: CapturedHeaders,
    captured_bodies: CapturedBodies,
    _temp_dir: TempDir,
}
include!("support/validation.rs");
include!("support/upstream.rs");
include!("support/text_helpers.rs");
include!("support/setup.rs");

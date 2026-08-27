use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::response::Sse;
use axum::response::sse::Event;
use axum::routing::post;
use base64::Engine as _;
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
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
    _temp_dir: TestTempDir,
}

struct TestTempDir(Option<TempDir>);

impl Deref for TestTempDir {
    type Target = TempDir;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("test temporary directory exists")
    }
}

static TEST_CLEANUP_FAILURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn test_cleanup_failure() -> &'static Mutex<Option<String>> {
    TEST_CLEANUP_FAILURE.get_or_init(|| Mutex::new(None))
}

fn assert_test_cleanup_succeeded() {
    let failure = test_cleanup_failure()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(error) = failure {
        panic!("earlier test database cleanup failed: {error}");
    }
}

fn close_test_state(state: &monoize::app::AppState, temp_dir: Option<TempDir>) {
    state.background_shutdown.store(true, Ordering::Release);
    let db = state.db_pool.clone();

    // Do not join here. A request-log task on the current test runtime can still
    // hold a connection until TestContext::drop returns and that runtime stops.
    let cleanup = std::thread::Builder::new()
        .name("monoize-test-db-close".to_string())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| runtime.block_on(db.close()).map_err(|error| error.to_string()));
            drop(temp_dir);
            if let Err(error) = result {
                *test_cleanup_failure()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
            }
        });
    if let Err(error) = cleanup {
        if std::thread::panicking() {
            eprintln!("test database cleanup failed during panic: {error}");
        } else {
            panic!("start test database cleanup thread: {error}");
        }
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        close_test_state(&self.state, self._temp_dir.0.take());
    }
}
include!("support/validation.rs");
include!("support/upstream.rs");
include!("support/text_helpers.rs");
include!("support/setup.rs");

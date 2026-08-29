//! QuickJS sandbox execution (`custom-js-transforms.spec.md` §7, §8).
//!
//! Every invocation builds a fresh single-threaded QuickJS runtime on the
//! blocking thread pool, evaluates the full source, and calls the global
//! `transform(ctx)` once. The only host bridges are `Monoize.fetch` and the
//! four `console` logging functions; there is no filesystem, process, timer,
//! or module-loading API.

use crate::transforms::Phase;
use rquickjs::{Context, Ctx, Exception, Function, Runtime};
use serde_json::{Value, json};
use std::cell::Cell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SandboxLimits {
    pub memory_limit_bytes: usize,
    pub stack_limit_bytes: usize,
    pub timeout: Duration,
    pub fetch_max_bytes: usize,
    pub fetch_max_calls: u32,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 32 * 1024 * 1024,
            stack_limit_bytes: 1024 * 1024,
            timeout: Duration::from_millis(10_000),
            fetch_max_bytes: 8 * 1024 * 1024,
            fetch_max_calls: 16,
        }
    }
}

impl SandboxLimits {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            memory_limit_bytes: env_usize(
                "MONOIZE_CUSTOM_JS_MEMORY_LIMIT_BYTES",
                defaults.memory_limit_bytes,
            ),
            stack_limit_bytes: env_usize(
                "MONOIZE_CUSTOM_JS_STACK_LIMIT_BYTES",
                defaults.stack_limit_bytes,
            ),
            timeout: Duration::from_millis(env_u64(
                "MONOIZE_CUSTOM_JS_TIMEOUT_MS",
                defaults.timeout.as_millis() as u64,
            )),
            fetch_max_bytes: env_usize(
                "MONOIZE_CUSTOM_JS_FETCH_MAX_BYTES",
                defaults.fetch_max_bytes,
            ),
            fetch_max_calls: env_u64(
                "MONOIZE_CUSTOM_JS_FETCH_MAX_CALLS",
                defaults.fetch_max_calls as u64,
            ) as u32,
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// CJS-EX-3: process-wide invocation concurrency bound.
fn sandbox_semaphore() -> &'static tokio::sync::Semaphore {
    static SEMAPHORE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    SEMAPHORE.get_or_init(|| {
        let permits = env_u64("MONOIZE_CUSTOM_JS_MAX_CONCURRENCY", 8) as usize;
        tokio::sync::Semaphore::new(permits.max(1))
    })
}

/// One `transform(ctx)` invocation request.
pub struct SandboxInvocation {
    pub transform_id: String,
    pub source: String,
    /// `"request"`, `"response"`, or `"stream"` per CJS-JS-1.
    pub kind: &'static str,
    pub phase: Phase,
    pub data: Value,
    pub config: Value,
    pub state: Value,
    pub upstream_provider_type: Option<String>,
}

/// Payload disposition after a successful invocation (CJS-JS-4, CJS-JS-6).
#[derive(Debug)]
pub enum SandboxData {
    Single(Value),
    Dropped,
    Fanout(Vec<Value>),
}

#[derive(Debug)]
pub struct SandboxOutcome {
    pub data: SandboxData,
    pub state: Value,
}

/// Glue evaluated after the transform source. It receives the ctx JSON string,
/// dispatches to the global `transform`, classifies the return value, and
/// serializes the outcome together with the final `ctx.state`.
const INVOKE_GLUE: &str = r#"(function (ctxJson) {
  "use strict";
  var ctx = JSON.parse(ctxJson);
  if (typeof transform !== "function") {
    throw new Error("global 'transform' is not a function");
  }
  var ret = transform(ctx);
  var out;
  if (ret === undefined) {
    out = { t: "data", data: ctx.data };
  } else if (ret === null) {
    out = { t: "null" };
  } else if (Array.isArray(ret)) {
    out = { t: "array", items: ret };
  } else if (typeof ret === "object") {
    out = { t: "data", data: ret };
  } else {
    out = { t: "invalid", type: typeof ret };
  }
  return JSON.stringify({ out: out, state: ctx.state === undefined ? {} : ctx.state });
})"#;

/// Glue for save-time validation (CJS-VAL-3, CJS-VAL-4).
const VALIDATE_GLUE: &str = r#"(function () {
  "use strict";
  if (typeof transform !== "function") {
    return JSON.stringify({ ok: false, error: "global 'transform' is not a function" });
  }
  if (typeof configSchema === "undefined") {
    return JSON.stringify({ ok: true, schema: null });
  }
  var serialized;
  try {
    serialized = JSON.stringify(configSchema);
  } catch (e) {
    return JSON.stringify({ ok: false, error: "configSchema is not JSON-serializable" });
  }
  if (typeof serialized !== "string") {
    return JSON.stringify({ ok: false, error: "configSchema is not JSON-serializable" });
  }
  return JSON.stringify({ ok: true, schema: serialized });
})"#;

/// Host bridge glue: wraps the raw JSON-string host functions into the
/// documented `Monoize.fetch` and `console` APIs (CJS-JS-9, CJS-JS-10).
const BRIDGE_GLUE: &str = r#"(function () {
  "use strict";
  // Capture the raw host functions, then remove the globals so scripts can
  // only reach the hosts through the documented wrappers.
  var hostLog = globalThis.__monoize_host_log;
  var hostFetch = globalThis.__monoize_host_fetch;
  delete globalThis.__monoize_host_log;
  delete globalThis.__monoize_host_fetch;
  function formatArg(value) {
    if (typeof value === "string") return value;
    try {
      var s = JSON.stringify(value);
      return s === undefined ? String(value) : s;
    } catch (e) {
      return String(value);
    }
  }
  function makeLog(level) {
    return function () {
      var parts = [];
      for (var i = 0; i < arguments.length; i++) parts.push(formatArg(arguments[i]));
      hostLog(level, parts.join(" "));
    };
  }
  globalThis.console = {
    log: makeLog("log"),
    info: makeLog("info"),
    warn: makeLog("warn"),
    error: makeLog("error")
  };
  globalThis.Monoize = {
    fetch: function (url, options) {
      var request = { url: String(url), options: options === undefined ? {} : options };
      return JSON.parse(hostFetch(JSON.stringify(request)));
    }
  };
})()"#;

/// Runs the full source and `transform(ctx)` under §7 resource bounds.
/// `http` carries the shared runtime HTTP client; `None` disables fetch
/// (validation mode per CJS-VAL-3).
pub async fn run_transform(
    invocation: SandboxInvocation,
    http: Option<reqwest::Client>,
    limits: SandboxLimits,
) -> Result<SandboxOutcome, String> {
    let _permit = sandbox_semaphore()
        .acquire()
        .await
        .map_err(|_| "sandbox semaphore closed".to_string())?;
    let handle = tokio::runtime::Handle::current();
    let kind = invocation.kind;
    let raw = tokio::task::spawn_blocking(move || {
        run_invocation_blocking(&invocation, http, &limits, handle)
    })
    .await
    .map_err(|error| format!("sandbox task failed: {error}"))??;

    parse_outcome(kind, &raw)
}

/// Save-time validation (CJS-VAL-3). Returns the declared `configSchema`
/// object when present.
pub async fn validate_source(
    source: String,
    limits: SandboxLimits,
) -> Result<Option<Value>, String> {
    let _permit = sandbox_semaphore()
        .acquire()
        .await
        .map_err(|_| "sandbox semaphore closed".to_string())?;
    let handle = tokio::runtime::Handle::current();
    let raw =
        tokio::task::spawn_blocking(move || run_validation_blocking(&source, &limits, handle))
            .await
            .map_err(|error| format!("sandbox task failed: {error}"))??;

    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("sandbox produced invalid validation JSON: {error}"))?;
    if parsed["ok"] != json!(true) {
        return Err(parsed["error"]
            .as_str()
            .unwrap_or("script validation failed")
            .to_string());
    }
    match parsed["schema"].as_str() {
        None => Ok(None),
        Some(serialized) => {
            let schema: Value = serde_json::from_str(serialized)
                .map_err(|error| format!("configSchema is not valid JSON: {error}"))?;
            if !schema.is_object() {
                return Err("configSchema must evaluate to a JSON object".to_string());
            }
            Ok(Some(schema))
        }
    }
}

fn parse_outcome(kind: &'static str, raw: &str) -> Result<SandboxOutcome, String> {
    let mut parsed: Value = serde_json::from_str(raw)
        .map_err(|error| format!("sandbox produced invalid outcome JSON: {error}"))?;
    let state = parsed
        .get_mut("state")
        .map(Value::take)
        .unwrap_or_else(|| json!({}));
    let out = parsed
        .get_mut("out")
        .map(Value::take)
        .ok_or_else(|| "sandbox outcome is missing 'out'".to_string())?;
    let tag = out["t"].as_str().unwrap_or("invalid");

    let data = match (kind, tag) {
        (_, "data") => {
            let data = out
                .get("data")
                .cloned()
                .ok_or_else(|| "sandbox outcome is missing 'data'".to_string())?;
            if !data.is_object() {
                return Err("transform result payload must be an object".to_string());
            }
            SandboxData::Single(data)
        }
        ("stream", "null") => SandboxData::Dropped,
        ("stream", "array") => {
            let items = out["items"]
                .as_array()
                .cloned()
                .ok_or_else(|| "sandbox outcome is missing 'items'".to_string())?;
            if items.iter().any(|item| !item.is_object()) {
                return Err("every element of a stream fan-out array must be an object".to_string());
            }
            SandboxData::Fanout(items)
        }
        (_, "null") => {
            return Err(format!(
                "transform returned null; a {kind} transform must return undefined or an object"
            ));
        }
        (_, "array") => {
            return Err(format!(
                "transform returned an array; a {kind} transform must return undefined or an object"
            ));
        }
        (_, other_tag) => {
            let type_name = out["type"].as_str().unwrap_or(other_tag);
            return Err(format!(
                "transform returned a value of type '{type_name}'; expected undefined or an object"
            ));
        }
    };

    Ok(SandboxOutcome { data, state })
}

fn run_invocation_blocking(
    invocation: &SandboxInvocation,
    http: Option<reqwest::Client>,
    limits: &SandboxLimits,
    handle: tokio::runtime::Handle,
) -> Result<String, String> {
    let deadline = Instant::now() + limits.timeout;
    let (runtime, context) = build_sandbox(limits, deadline)?;
    let ctx_json = serde_json::to_string(&json!({
        "phase": invocation.phase,
        "kind": invocation.kind,
        "data": invocation.data,
        "config": invocation.config,
        "state": invocation.state,
        "upstream_provider_type": invocation.upstream_provider_type,
    }))
    .map_err(|error| format!("failed to serialize sandbox context: {error}"))?;

    let result = context.with(|ctx| -> Result<String, String> {
        install_host_bridges(
            &ctx,
            &invocation.transform_id,
            http,
            limits,
            deadline,
            handle,
        )?;
        eval_source(&ctx, &invocation.source)?;
        let glue: Function = ctx
            .eval(INVOKE_GLUE)
            .map_err(|error| describe_js_error(&ctx, error, "internal glue"))?;
        glue.call((ctx_json.as_str(),))
            .map_err(|error| describe_js_error(&ctx, error, "transform invocation"))
    });
    drop(context);
    drop(runtime);
    result
}

fn run_validation_blocking(
    source: &str,
    limits: &SandboxLimits,
    handle: tokio::runtime::Handle,
) -> Result<String, String> {
    let deadline = Instant::now() + limits.timeout;
    let (runtime, context) = build_sandbox(limits, deadline)?;
    let result = context.with(|ctx| -> Result<String, String> {
        // CJS-VAL-3: fetch is installed but always throws during validation.
        install_host_bridges(&ctx, "validation", None, limits, deadline, handle)?;
        eval_source(&ctx, source)?;
        let glue: Function = ctx
            .eval(VALIDATE_GLUE)
            .map_err(|error| describe_js_error(&ctx, error, "internal glue"))?;
        glue.call(())
            .map_err(|error| describe_js_error(&ctx, error, "validation"))
    });
    drop(context);
    drop(runtime);
    result
}

fn build_sandbox(limits: &SandboxLimits, deadline: Instant) -> Result<(Runtime, Context), String> {
    let runtime = Runtime::new().map_err(|error| format!("sandbox runtime init: {error}"))?;
    runtime.set_memory_limit(limits.memory_limit_bytes);
    runtime.set_max_stack_size(limits.stack_limit_bytes);
    // CJS-EX-2: the interrupt handler enforces the wall-clock budget for pure
    // JS execution; host fetch time is bounded separately by capping each
    // fetch timeout to the remaining budget.
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context =
        Context::full(&runtime).map_err(|error| format!("sandbox context init: {error}"))?;
    Ok((runtime, context))
}

fn eval_source(ctx: &Ctx<'_>, source: &str) -> Result<(), String> {
    ctx.eval::<(), _>(source)
        .map_err(|error| describe_js_error(ctx, error, "script evaluation"))
}

fn install_host_bridges(
    ctx: &Ctx<'_>,
    transform_id: &str,
    http: Option<reqwest::Client>,
    limits: &SandboxLimits,
    deadline: Instant,
    handle: tokio::runtime::Handle,
) -> Result<(), String> {
    let log_target = transform_id.to_string();
    let log_fn = Function::new(ctx.clone(), move |level: String, message: String| {
        emit_console_log(&log_target, &level, &message);
    })
    .map_err(|error| format!("failed to install console bridge: {error}"))?;
    ctx.globals()
        .set("__monoize_host_log", log_fn)
        .map_err(|error| format!("failed to install console bridge: {error}"))?;

    let fetch_max_bytes = limits.fetch_max_bytes;
    let fetch_max_calls = limits.fetch_max_calls;
    let calls = Cell::new(0u32);
    let fetch_fn = Function::new(
        ctx.clone(),
        move |fctx: Ctx<'_>, request_json: String| -> rquickjs::Result<String> {
            let Some(client) = http.as_ref() else {
                return Err(Exception::throw_message(
                    &fctx,
                    "fetch is not available during validation",
                ));
            };
            let call_index = calls.get() + 1;
            if call_index > fetch_max_calls {
                return Err(Exception::throw_message(
                    &fctx,
                    &format!(
                        "Monoize.fetch call limit exceeded ({fetch_max_calls} per invocation)"
                    ),
                ));
            }
            calls.set(call_index);
            match host_fetch(client, &handle, &request_json, deadline, fetch_max_bytes) {
                Ok(response_json) => Ok(response_json),
                Err(message) => Err(Exception::throw_message(&fctx, &message)),
            }
        },
    )
    .map_err(|error| format!("failed to install fetch bridge: {error}"))?;
    ctx.globals()
        .set("__monoize_host_fetch", fetch_fn)
        .map_err(|error| format!("failed to install fetch bridge: {error}"))?;

    ctx.eval::<(), _>(BRIDGE_GLUE)
        .map_err(|error| describe_js_error(ctx, error, "host bridge glue"))?;
    Ok(())
}

fn emit_console_log(transform_id: &str, level: &str, message: &str) {
    match level {
        "warn" => tracing::warn!(target: "monoize::custom_transform", transform_id, "{message}"),
        "error" => tracing::error!(target: "monoize::custom_transform", transform_id, "{message}"),
        _ => tracing::info!(target: "monoize::custom_transform", transform_id, "{message}"),
    }
}

/// One blocking HTTP round-trip through the shared client (CJS-JS-9).
fn host_fetch(
    client: &reqwest::Client,
    handle: &tokio::runtime::Handle,
    request_json: &str,
    deadline: Instant,
    max_bytes: usize,
) -> Result<String, String> {
    let request: Value = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid fetch request: {error}"))?;
    let url = request["url"]
        .as_str()
        .ok_or_else(|| "fetch url must be a string".to_string())?
        .to_string();
    let options = &request["options"];
    if !options.is_object() {
        return Err("fetch options must be an object".to_string());
    }
    let method = match options.get("method").and_then(Value::as_str) {
        None => reqwest::Method::GET,
        Some(raw) => raw
            .to_ascii_uppercase()
            .parse::<reqwest::Method>()
            .map_err(|_| format!("invalid fetch method '{raw}'"))?,
    };
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| "invocation time budget exhausted before fetch".to_string())?;
    // Effective timeout = min(options.timeout_ms, remaining budget).
    let timeout = match options.get("timeout_ms").and_then(Value::as_u64) {
        Some(ms) if ms > 0 => remaining.min(Duration::from_millis(ms)),
        _ => remaining,
    };

    let mut builder = client.request(method, &url).timeout(timeout);
    if let Some(headers) = options.get("headers").and_then(Value::as_object) {
        for (name, value) in headers {
            let value = value
                .as_str()
                .ok_or_else(|| format!("fetch header '{name}' must be a string"))?;
            builder = builder.header(name, value);
        }
    }
    if let Some(body) = options.get("body") {
        if !body.is_null() {
            let body = body
                .as_str()
                .ok_or_else(|| "fetch body must be a string".to_string())?;
            builder = builder.body(body.to_string());
        }
    }

    let response = handle.block_on(async move {
        let response = builder
            .send()
            .await
            .map_err(|error| format!("fetch failed: {error}"))?;
        let status = response.status().as_u16();
        let mut headers = serde_json::Map::new();
        for (name, value) in response.headers() {
            let value = value
                .to_str()
                .map_err(|_| format!("fetch response header '{name}' is not valid UTF-8"))?;
            let key = name.as_str().to_ascii_lowercase();
            match headers.get_mut(&key) {
                Some(Value::String(existing)) => {
                    existing.push_str(", ");
                    existing.push_str(value);
                }
                _ => {
                    headers.insert(key, Value::String(value.to_string()));
                }
            }
        }
        let mut body = Vec::new();
        let mut stream = response;
        while let Some(chunk) = stream
            .chunk()
            .await
            .map_err(|error| format!("fetch body read failed: {error}"))?
        {
            if body.len() + chunk.len() > max_bytes {
                return Err(format!("fetch response body exceeds {max_bytes} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        let body = String::from_utf8(body)
            .map_err(|_| "fetch response body is not valid UTF-8".to_string())?;
        Ok::<_, String>(json!({
            "status": status,
            "headers": Value::Object(headers),
            "body": body,
        }))
    })?;

    serde_json::to_string(&response)
        .map_err(|error| format!("failed to serialize fetch response: {error}"))
}

/// Converts an rquickjs error into a stable, human-readable message that
/// includes the thrown JS value when one is pending on the context.
fn describe_js_error(ctx: &Ctx<'_>, error: rquickjs::Error, stage: &str) -> String {
    if matches!(error, rquickjs::Error::Exception) {
        let caught = ctx.catch();
        let message = if let Some(exception) = caught.as_exception() {
            let text = exception.message().unwrap_or_default();
            if text.is_empty() {
                "unknown exception".to_string()
            } else {
                text
            }
        } else {
            stringify_js_value(ctx, &caught)
        };
        return format!("{stage} threw: {message}");
    }
    format!("{stage} failed: {error}")
}

fn stringify_js_value<'js>(ctx: &Ctx<'js>, value: &rquickjs::Value<'js>) -> String {
    ctx.json_stringify(value.clone())
        .ok()
        .flatten()
        .and_then(|text| text.to_string().ok())
        .unwrap_or_else(|| "unknown exception".to_string())
}

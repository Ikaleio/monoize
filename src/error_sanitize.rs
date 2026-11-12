//! Client-facing error masking per `spec/upstream-error-sanitization.spec.md`.
//!
//! The masking algorithm follows the New API reference implementation
//! (`relaykit/relayconvert/kitutil/mask.go` in QuantumNous/new-api): URLs,
//! bare domain names, IPv4 addresses, and `api_key:` values are masked before
//! upstream-derived error text reaches API clients, and again at read time
//! when a non-admin dashboard user views persisted request-log error detail
//! (SAN-14). Persisted request-log fields keep the raw truncated detail for
//! admin visibility; the server tracing log keeps the unbounded raw detail.

use regex::Regex;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{LazyLock, RwLock};

/// SAN-D2: maximum Unicode scalar values retained by [`truncate_error_detail`].
pub const ERROR_DETAIL_MAX_CHARS: usize = 2048;
const TRUNCATION_SUFFIX: &str = "... (truncated)";

static URL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(https?|wss?)://[^\s/$.?#].[^\s]*").expect("valid URL regex"));
static DOMAIN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:[\p{L}\p{N}](?:[\p{L}\p{N}-]{0,61}[\p{L}\p{N}])?\.)+(?:xn--[a-zA-Z0-9-]{1,59}|\p{L}{2,})\b",
    )
    .expect("valid domain regex")
});
static IP_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").expect("valid IPv4 regex"));
static API_KEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(['"]?)api_key:([^\s'"]+)(['"]?)"#).expect("valid api_key regex")
});

/// SAN-D1 `TLDS`: IANA root-zone snapshot plus non-delegated infrastructure names.
static TLDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("error_sanitize_tlds.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .chain([
            "cluster",
            "corp",
            "example",
            "home",
            "internal",
            "intranet",
            "invalid",
            "lan",
            "local",
            "localhost",
            "onion",
            "svc",
            "test",
        ])
        .collect()
});
const FIELD_LEAVES: &[&str] = &[
    "audio", "call", "data", "delta", "events", "id", "name", "parts", "store", "stream", "tools",
    "total",
];
const FIELD_ROOTS: &[&str] = &[
    "arguments",
    "body",
    "call",
    "choice",
    "choices",
    "content",
    "contents",
    "data",
    "delta",
    "error",
    "function",
    "functions",
    "input",
    "instructions",
    "item",
    "items",
    "message",
    "messages",
    "metadata",
    "model",
    "output",
    "parameters",
    "params",
    "parts",
    "properties",
    "reasoning",
    "request",
    "response",
    "responses",
    "schema",
    "source",
    "stream",
    "system",
    "text",
    "thinking",
    "tool",
    "tools",
    "usage",
];
/// SAN-D1 step 2: upper bound on `CONFIGURED` hosts.
const CONFIGURED_HOSTS_MAX: usize = 4096;

#[derive(Default)]
struct ConfiguredHosts {
    seen_urls: HashSet<String>,
    hosts: HashSet<String>,
    dotted: Option<Regex>,
    single_label: Option<Regex>,
}

static CONFIGURED: LazyLock<RwLock<ConfiguredHosts>> = LazyLock::new(RwLock::default);

fn alternation_regex(hosts: &[&String], template: impl Fn(&str) -> String) -> Option<Regex> {
    if hosts.is_empty() {
        return None;
    }
    let alternation = hosts
        .iter()
        .map(|host| regex::escape(host))
        .collect::<Vec<_>>()
        .join("|");
    match Regex::new(&template(&alternation)) {
        Ok(pattern) => Some(pattern),
        Err(err) => {
            tracing::error!(error = %err, "configured host mask pattern failed to compile");
            None
        }
    }
}

/// SAN-D1 step 2: adds the host of an upstream or proxy URL to `CONFIGURED`.
/// Repeat calls with a known URL take only the read lock, because attempt
/// construction calls this on every request.
pub fn register_configured_url(url: &str) {
    let seen = CONFIGURED
        .read()
        .expect("configured host lock is never poisoned")
        .seen_urls
        .contains(url);
    if seen {
        return;
    }
    let mut configured = CONFIGURED
        .write()
        .expect("configured host lock is never poisoned");
    if configured.hosts.len() >= CONFIGURED_HOSTS_MAX {
        return;
    }
    configured.seen_urls.insert(url.to_string());
    let Some(host) = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.domain().map(str::to_ascii_lowercase))
    else {
        return;
    };
    if !configured.hosts.insert(host) {
        return;
    }
    let (dotted, single_label): (Vec<&String>, Vec<&String>) =
        configured.hosts.iter().partition(|host| host.contains('.'));
    let dotted = alternation_regex(&dotted, |hosts| {
        format!(r"(?i)(?:[\p{{L}}\p{{N}}-]+\.)*(?:{hosts})\b")
    });
    let single_label =
        alternation_regex(&single_label, |hosts| format!(r"(?i)\b(?:{hosts})(:\d+)"));
    configured.dotted = dotted;
    configured.single_label = single_label;
}

fn field_root(label: &str) -> bool {
    FIELD_ROOTS
        .iter()
        .any(|root| root.eq_ignore_ascii_case(label))
}

/// SAN-D1 `PROTOCOL_PATH`: a field path such as `messages.0.name`.
fn is_protocol_path(candidate: &str) -> bool {
    let mut labels = candidate.split('.');
    let Some(last) = labels.next_back() else {
        return false;
    };
    FIELD_LEAVES
        .iter()
        .any(|leaf| leaf.eq_ignore_ascii_case(last))
        && labels.next().is_some_and(field_root)
        && labels.all(|label| field_root(label) || label.bytes().all(|byte| byte.is_ascii_digit()))
}

/// SAN-D1 `IS_HOST`: separates hostnames from dotted protocol identifiers such
/// as `response.create` or `messages.1.content.0.type`.
fn is_host(candidate: &str) -> bool {
    if candidate.split('.').any(|label| {
        label
            .get(..4)
            .is_some_and(|p| p.eq_ignore_ascii_case("xn--"))
    }) {
        return true;
    }
    let last = candidate.rsplit('.').next().unwrap_or(candidate);
    if !last.is_ascii() {
        return true;
    }
    let last: Cow<'_, str> = if last.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(last.to_ascii_lowercase())
    } else {
        Cow::Borrowed(last)
    };
    TLDS.contains(last.as_ref()) && !is_protocol_path(candidate)
}

/// SAN-D1 `MASK`: mask URLs, bare domains, IPv4 addresses, and `api_key:`
/// values in `text`. Idempotent: masked output contains no residual match for
/// the domain/IP/api_key patterns, and re-masking an already-masked URL is a
/// fixed point.
pub fn mask_sensitive_text(text: &str) -> String {
    let masked = URL_PATTERN.replace_all(text, |captures: &regex::Captures<'_>| {
        mask_url(&captures[0])
    });
    let masked = mask_configured_hosts(masked);
    let masked = DOMAIN_PATTERN.replace_all(&masked, |captures: &regex::Captures<'_>| {
        if is_host(&captures[0]) && !already_masked(captures, &masked) {
            mask_host_for_plain_domain(&captures[0])
        } else {
            captures[0].to_string()
        }
    });
    let masked = IP_PATTERN.replace_all(&masked, "***.***.***.***");
    API_KEY_PATTERN
        .replace_all(&masked, "${1}api_key:***${3}")
        .into_owned()
}

/// A tail kept by `MASKHOST` (`***.bar.io`) still matches the host patterns;
/// re-masking it would break the idempotence SAN-15 relies on.
fn already_masked(captures: &regex::Captures<'_>, text: &str) -> bool {
    let start = captures.get(0).map_or(0, |matched| matched.start());
    text[..start].ends_with("***.")
}

/// SAN-D1 step 2. Runs before the bare-domain step, whose exemptions do not
/// apply to a host Monoize itself connects to.
fn mask_configured_hosts(text: Cow<'_, str>) -> Cow<'_, str> {
    let configured = CONFIGURED
        .read()
        .expect("configured host lock is never poisoned");
    let mut text = text;
    if let Some(pattern) = &configured.dotted {
        let replaced = pattern.replace_all(&text, |captures: &regex::Captures<'_>| {
            if already_masked(captures, &text) {
                captures[0].to_string()
            } else {
                mask_host_for_plain_domain(&captures[0])
            }
        });
        if let Cow::Owned(replaced) = replaced {
            text = Cow::Owned(replaced);
        }
    }
    if let Some(pattern) = &configured.single_label
        && let Cow::Owned(replaced) = pattern.replace_all(&text, "***${1}")
    {
        text = Cow::Owned(replaced);
    }
    text
}

/// SAN-CFG5: apply [`mask_sensitive_text`] only when the runtime setting
/// `mask_sensitive_info` is enabled; otherwise return `text` unchanged.
pub fn maybe_mask_sensitive_text(text: &str, mask_sensitive_info: bool) -> String {
    if mask_sensitive_info {
        mask_sensitive_text(text)
    } else {
        text.to_string()
    }
}

/// SAN-D2 `TRUNC`: bound persisted error detail to [`ERROR_DETAIL_MAX_CHARS`]
/// Unicode scalar values, appending a fixed truncation marker when clipped.
pub fn truncate_error_detail(text: &str) -> String {
    match text.char_indices().nth(ERROR_DETAIL_MAX_CHARS) {
        None => text.to_string(),
        Some((byte_index, _)) => format!("{}{}", &text[..byte_index], TRUNCATION_SUFFIX),
    }
}

/// SAN-D1 tail rule: keep two labels for likely country-code TLDs
/// (e.g. `co.uk`, `com.cn`), otherwise keep only the TLD.
fn preserved_tail_len(parts: &[&str]) -> usize {
    if parts.len() < 2 {
        return parts.len();
    }
    let last = parts[parts.len() - 1];
    let second_last = parts[parts.len() - 2];
    if last.len() == 2 && second_last.len() <= 3 {
        2
    } else {
        1
    }
}

/// `MASKHOST`: collapse all subdomain labels into one `***.` prefix.
fn mask_host_for_url(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 2 {
        return "***".to_string();
    }
    let tail_len = preserved_tail_len(&parts);
    format!("***.{}", parts[parts.len() - tail_len..].join("."))
}

/// Bare-domain masking reflects subdomain depth with repeated `***.` labels
/// (e.g. `api.openai.com` becomes `***.***.com`).
fn mask_host_for_plain_domain(domain: &str) -> String {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() < 2 {
        return domain.to_string();
    }
    let tail_len = preserved_tail_len(&parts);
    let star_count = (parts.len() - tail_len).max(1);
    format!(
        "{}{}",
        "***.".repeat(star_count),
        parts[parts.len() - tail_len..].join(".")
    )
}

fn mask_url(url_str: &str) -> String {
    let Ok(url) = reqwest::Url::parse(url_str) else {
        return url_str.to_string();
    };
    let Some(host) = url.host_str() else {
        return url_str.to_string();
    };

    let mut result = format!("{}://{}", url.scheme(), mask_host_for_url(host));
    if let Some(port) = url.port() {
        result.push_str(&format!(":{port}"));
    }

    let path = url.path();
    if !path.is_empty() && path != "/" {
        let masked_segments: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .map(|segment| if segment.is_empty() { "" } else { "***" })
            .collect();
        result.push('/');
        result.push_str(&masked_segments.join("/"));
    } else if path == "/" {
        result.push('/');
    }

    if url.query().is_some_and(|query| !query.is_empty()) {
        let masked_params: Vec<String> = url
            .query_pairs()
            .map(|(key, _)| format!("{key}=***"))
            .collect();
        if masked_params.is_empty() {
            result.push_str("?***");
        } else {
            result.push('?');
            result.push_str(&masked_params.join("&"));
        }
    }

    result
}

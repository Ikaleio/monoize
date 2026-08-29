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
use std::sync::LazyLock;

/// SAN-D2: maximum Unicode scalar values retained by [`truncate_error_detail`].
pub const ERROR_DETAIL_MAX_CHARS: usize = 2048;
const TRUNCATION_SUFFIX: &str = "... (truncated)";

static URL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(http|https)://[^\s/$.?#].[^\s]*").expect("valid URL regex"));
static DOMAIN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,}\b")
        .expect("valid domain regex")
});
static IP_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").expect("valid IPv4 regex"));
static API_KEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(['"]?)api_key:([^\s'"]+)(['"]?)"#).expect("valid api_key regex")
});

/// SAN-D1 `MASK`: mask URLs, bare domains, IPv4 addresses, and `api_key:`
/// values in `text`. Idempotent: masked output contains no residual match for
/// the domain/IP/api_key patterns, and re-masking an already-masked URL is a
/// fixed point.
pub fn mask_sensitive_text(text: &str) -> String {
    let masked = URL_PATTERN.replace_all(text, |captures: &regex::Captures<'_>| {
        mask_url(&captures[0])
    });
    let masked = DOMAIN_PATTERN.replace_all(&masked, |captures: &regex::Captures<'_>| {
        mask_host_for_plain_domain(&captures[0])
    });
    let masked = IP_PATTERN.replace_all(&masked, "***.***.***.***");
    API_KEY_PATTERN
        .replace_all(&masked, "${1}api_key:***${3}")
        .into_owned()
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

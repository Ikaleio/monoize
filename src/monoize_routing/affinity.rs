pub(super) const SESSION_CACHE_HEADER_NAMES: &[&str] =
    &["x-session-affinity", "x-opencode-session"];

/// CM-AFF-0: null `session_affinity_auto` enables affinity for these URLs.
pub(crate) fn default_session_affinity_auto(base_url: &str) -> bool {
    is_direct_cloudflare_workers_ai_url(base_url) || is_direct_opencode_zen_url(base_url)
}

/// CM-AFF-0: `https://api.cloudflare.com/client/v4/accounts/{id}/ai` or `.../ai/v1`.
fn is_direct_cloudflare_workers_ai_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("api.cloudflare.com")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }

    let path = url.path().strip_suffix('/').unwrap_or(url.path());
    let Some(account_and_suffix) = path.strip_prefix("/client/v4/accounts/") else {
        return false;
    };
    let account_id = account_and_suffix
        .strip_suffix("/ai/v1")
        .or_else(|| account_and_suffix.strip_suffix("/ai"));
    account_id.is_some_and(|account_id| !account_id.is_empty() && !account_id.contains('/'))
}

/// CM-AFF-0b: `https://opencode.ai/zen` and paths under `/zen/`.
pub(super) fn is_direct_opencode_zen_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("opencode.ai")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let path = url.path().strip_suffix('/').unwrap_or(url.path());
    path == "/zen" || path.starts_with("/zen/")
}

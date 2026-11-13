use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use dashmap::DashMap;
use percent_encoding::percent_decode_str;

type ProxyCredentials<'a> = (Cow<'a, str>, Cow<'a, str>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    Primary,
    Replica,
}

impl NodeRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeRole::Primary => "primary",
            NodeRole::Replica => "replica",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeSettings {
    pub role: NodeRole,
    pub replica_primary_url: Option<String>,
    pub replica_token: Option<String>,
    /// Optional fixed replica identity (`MONOIZE_REPLICA_ID`); validated per M9 on replicas.
    pub replica_id: Option<String>,
    pub upstream_proxy_url: Option<String>,
    pub config_poll_interval: Duration,
    pub metering_ship_interval: Duration,
    pub metering_ship_batch_max_entries: usize,
    pub metering_spool_dir: PathBuf,
    pub metering_spool_max_bytes: u64,
}

const DEFAULT_CONFIG_POLL_INTERVAL_SECONDS: u64 = 5;
const DEFAULT_METERING_SHIP_INTERVAL_SECONDS: u64 = 10;
const DEFAULT_METERING_SHIP_BATCH_MAX_ENTRIES: usize = 500;
const DEFAULT_METERING_SPOOL_DIR: &str = "./data/replica-metering-spool";
const DEFAULT_METERING_SPOOL_MAX_BYTES: u64 = 536_870_912;

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn positive_seconds(
    get: &impl Fn(&str) -> Option<String>,
    name: &str,
    error_code: &'static str,
) -> Result<Option<u64>, (&'static str, String)> {
    let Some(raw) = get(name) else {
        return Ok(None);
    };
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(Some)
        .ok_or_else(|| {
            (
                error_code,
                format!("`{name}` must be a positive integer, got {raw:?}"),
            )
        })
}

impl NodeSettings {
    pub fn primary_default() -> Self {
        Self {
            role: NodeRole::Primary,
            replica_primary_url: None,
            replica_token: None,
            replica_id: None,
            upstream_proxy_url: None,
            config_poll_interval: Duration::from_secs(DEFAULT_CONFIG_POLL_INTERVAL_SECONDS),
            metering_ship_interval: Duration::from_secs(DEFAULT_METERING_SHIP_INTERVAL_SECONDS),
            metering_ship_batch_max_entries: DEFAULT_METERING_SHIP_BATCH_MAX_ENTRIES,
            metering_spool_dir: PathBuf::from(DEFAULT_METERING_SPOOL_DIR),
            metering_spool_max_bytes: DEFAULT_METERING_SPOOL_MAX_BYTES,
        }
    }

    /// Resolves node-local settings from the environment.
    /// The returned error is `(error_code, detail)` and MUST stop startup per PRP1/PRP7/PX2.
    pub fn from_env() -> Result<Self, (&'static str, String)> {
        Self::from_env_bindings(env_trimmed)
    }

    pub fn from_env_bindings(
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, (&'static str, String)> {
        let mut settings = Self::primary_default();

        if let Some(raw_role) = get("MONOIZE_NODE_ROLE") {
            settings.role = match raw_role.as_str() {
                "primary" => NodeRole::Primary,
                "replica" => NodeRole::Replica,
                _ => {
                    return Err((
                        "node_role_invalid",
                        format!(
                            "`MONOIZE_NODE_ROLE` must be `primary` or `replica`, got {raw_role:?}"
                        ),
                    ));
                }
            };
        }

        settings.replica_primary_url = get("MONOIZE_PRIMARY_INTERNAL_URL");
        settings.replica_token = get("MONOIZE_REPLICA_TOKEN");
        settings.replica_id = get("MONOIZE_REPLICA_ID");
        settings.upstream_proxy_url = get("MONOIZE_UPSTREAM_PROXY_URL");

        if let Some(seconds) = positive_seconds(
            &get,
            "MONOIZE_CONFIG_POLL_INTERVAL_SECONDS",
            "config_poll_interval_invalid",
        )? {
            settings.config_poll_interval = Duration::from_secs(seconds);
        }
        if let Some(seconds) = positive_seconds(
            &get,
            "MONOIZE_METERING_SHIP_INTERVAL_SECONDS",
            "metering_ship_interval_invalid",
        )? {
            settings.metering_ship_interval = Duration::from_secs(seconds);
        }
        if let Some(raw) = get("MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES") {
            let parsed = raw
                .parse::<usize>()
                .ok()
                .filter(|value| (1..=2000).contains(value))
                .ok_or_else(|| {
                    (
                        "metering_batch_limit_invalid",
                        format!(
                            "`MONOIZE_METERING_SHIP_BATCH_MAX_ENTRIES` must be an integer in [1, 2000], got {raw:?}"
                        ),
                    )
                })?;
            settings.metering_ship_batch_max_entries = parsed;
        }
        if let Some(dir) = get("MONOIZE_REPLICA_METERING_SPOOL_DIR") {
            settings.metering_spool_dir = PathBuf::from(dir);
        }
        if let Some(raw) = get("MONOIZE_REPLICA_METERING_SPOOL_MAX_BYTES") {
            let parsed = raw.parse::<u64>().ok().filter(|value| *value > 0).ok_or_else(|| {
                (
                    "metering_spool_quota_invalid",
                    format!(
                        "`MONOIZE_REPLICA_METERING_SPOOL_MAX_BYTES` must be a positive integer, got {raw:?}"
                    ),
                )
            })?;
            settings.metering_spool_max_bytes = parsed;
        }

        settings.validate()?;
        Ok(settings)
    }

    /// Cross-field validation per PRP3–PRP5 and PX2. `database_dsn` participates in the
    /// replica-requires-PostgreSQL rule because backend selection is DSN-driven (DB4).
    pub fn validate(&self) -> Result<(), (&'static str, String)> {
        if let Some(proxy_url) = &self.upstream_proxy_url {
            validate_http_proxy_url(proxy_url).map_err(|detail| {
                (
                    "upstream_proxy_config_invalid",
                    format!("`MONOIZE_UPSTREAM_PROXY_URL` {detail}"),
                )
            })?;
        }
        if self.role == NodeRole::Replica {
            // PRP3 is enforced by load_state where the DSN is available; see validate_for_dsn.
        }
        Ok(())
    }

    pub fn validate_for_dsn(&self, database_dsn: &str) -> Result<(), (&'static str, String)> {
        self.validate()?;
        if self
            .replica_token
            .as_deref()
            .is_some_and(|token| !token.is_empty() && token.chars().count() < 32)
        {
            return Err((
                "replica_token_too_short",
                "`MONOIZE_REPLICA_TOKEN` must contain at least 32 characters".to_string(),
            ));
        }
        if self.role == NodeRole::Replica {
            let lowered = database_dsn.to_ascii_lowercase();
            if lowered.starts_with("sqlite://")
                || lowered.starts_with("sqlite:")
                || lowered.contains(":memory:")
            {
                return Err((
                    "replica_requires_postgres",
                    "a replica node requires a PostgreSQL DSN; SQLite is not supported for replicas".to_string(),
                ));
            }
            let url_ok = self
                .replica_primary_url
                .as_deref()
                .and_then(|raw| reqwest::Url::parse(raw).ok())
                .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
            if !url_ok {
                return Err((
                    "replica_primary_url_required",
                    "`MONOIZE_PRIMARY_INTERNAL_URL` must be set to an absolute http(s) URL on a replica"
                        .to_string(),
                ));
            }
            if self.replica_token.as_deref().unwrap_or("").is_empty() {
                return Err((
                    "replica_token_required",
                    "`MONOIZE_REPLICA_TOKEN` must be set to a non-empty value on a replica"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn is_replica(&self) -> bool {
        self.role == NodeRole::Replica
    }
}

fn validate_http_proxy_url(raw: &str) -> Result<(), String> {
    let url = parse_proxy_url(raw)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("must use scheme http or https".into());
    }
    Ok(())
}

fn parse_proxy_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("is not a valid URL: {error}"))?;
    if url.host_str().is_none() {
        return Err("must contain a host".into());
    }
    Ok(url)
}

pub(crate) fn parse_channel_proxy_url(raw: &str) -> Result<reqwest::Url, String> {
    let mut url = parse_proxy_url(raw)?;
    let credentials = proxy_credentials(&url)?;
    match url.scheme() {
        "http" | "https" => {}
        "socks5" | "socks5h" => {
            if let Some((username, password)) = credentials
                && (!(1..=255).contains(&username.len()) || !(1..=255).contains(&password.len()))
            {
                return Err("SOCKS5 username and password must each contain 1–255 bytes".into());
            }
            if url.port().is_none() {
                url.set_port(Some(1080))
                    .map_err(|()| "cannot set the SOCKS5 proxy port")?;
            }
        }
        _ => return Err("must use scheme http, https, socks5, or socks5h".into()),
    }
    // Reqwest's proxy matcher requires an HTTP URI and skips URLs it cannot parse.
    url.as_str()
        .parse::<axum::http::Uri>()
        .map_err(|_| "is not a valid proxy URI")?;
    Ok(url)
}

pub(crate) fn proxy_credentials(
    url: &reqwest::Url,
) -> Result<Option<ProxyCredentials<'_>>, String> {
    let username = url.username();
    let password = url.password();
    if username.is_empty() && password.is_none() {
        return Ok(None);
    }
    let username = percent_decode_str(username)
        .decode_utf8()
        .map_err(|_| "proxy username must decode to UTF-8")?;
    let password = percent_decode_str(password.unwrap_or_default())
        .decode_utf8()
        .map_err(|_| "proxy password must decode to UTF-8")?;
    Ok(Some((username, password)))
}

struct SocksDnsResolver;

impl reqwest::dns::Resolve for SocksDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            // Reqwest passes bracketed IPv6 literals to the resolver for socks5.
            let host = name.as_str().trim_matches(['[', ']']);
            let addresses = tokio::net::lookup_host((host.to_owned(), 0)).await?;
            Ok(Box::new(addresses) as reqwest::dns::Addrs)
        })
    }
}

fn build_client(
    proxy: Option<reqwest::Proxy>,
    resolve_socks_locally: bool,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent("monoize/0.1")
        // PX4: internal callers rely on this builder bypassing environment-inherited proxies;
        // external callers get exactly one explicit proxy from configuration below.
        .no_proxy();
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy);
    }
    if resolve_socks_locally {
        builder = builder.dns_resolver(std::sync::Arc::new(SocksDnsResolver));
    }
    builder.build().map_err(|error| error.to_string())
}

/// Per-process HTTP client registry implementing PX3/PX6/PX7:
/// one global client honoring `MONOIZE_UPSTREAM_PROXY_URL`, one no-proxy internal client,
/// and an immutable cache of clients keyed by custom channel proxy URL.
#[derive(Clone)]
pub struct HttpClients {
    global: std::sync::Arc<reqwest::Client>,
    internal: std::sync::Arc<reqwest::Client>,
    per_proxy: std::sync::Arc<DashMap<String, std::sync::Arc<reqwest::Client>>>,
}

impl HttpClients {
    pub fn new(upstream_proxy_url: Option<&str>) -> Result<Self, String> {
        let proxy = upstream_proxy_url
            .map(reqwest::Proxy::all)
            .transpose()
            .map_err(|_| "cannot configure the node proxy")?;
        let global = std::sync::Arc::new(build_client(proxy, false)?);
        let internal = std::sync::Arc::new(build_client(None, false)?);
        Ok(Self {
            global,
            internal,
            per_proxy: std::sync::Arc::new(DashMap::new()),
        })
    }

    /// Client for calls that are part of Monoize's own cluster protocol (PX4/PX8).
    pub fn internal(&self) -> reqwest::Client {
        self.internal.as_ref().clone()
    }

    /// The node-global external client (PX3), used when a channel follows global.
    pub fn global_client(&self) -> reqwest::Client {
        self.global.as_ref().clone()
    }

    /// PX6 resolution: custom channel proxy wins, then the node-global client.
    /// Cached clients are immutable after construction; a changed channel URL simply
    /// resolves to a different cached entry on the next call. Construction failure
    /// fails closed instead of falling back to a different egress path.
    pub fn for_channel_proxy(&self, proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
        let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(self.global_client());
        };
        if let Some(entry) = self.per_proxy.get(proxy_url) {
            return Ok(entry.value().as_ref().clone());
        }
        let entry = self
            .per_proxy
            .entry(proxy_url.to_owned())
            .or_try_insert_with(|| {
                let url = parse_channel_proxy_url(proxy_url)?;
                let resolve_socks_locally = url.scheme() == "socks5";
                let proxy =
                    reqwest::Proxy::all(url).map_err(|_| "cannot configure the Channel proxy")?;
                build_client(Some(proxy), resolve_socks_locally).map(std::sync::Arc::new)
            })?;
        Ok(entry.value().as_ref().clone())
    }
}

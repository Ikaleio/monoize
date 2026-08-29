use bytes::BytesMut;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const CAP_API_ENDPOINT_ENV: &str = "MONOIZE_CAP_API_ENDPOINT";
const CAP_SECRET_KEY_ENV: &str = "MONOIZE_CAP_SECRET_KEY";
pub const BUILTIN_CAP_API_ENDPOINT: &str = "/api/dashboard/captcha/";
const VERIFY_RESPONSE_MAX_BYTES: usize = 4096;
const CAPTCHA_TOKEN_MAX_BYTES: usize = 4096;
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);
const BUILTIN_CHALLENGE_COUNT: usize = 50;
const BUILTIN_CHALLENGE_SIZE: usize = 32;
const BUILTIN_CHALLENGE_DIFFICULTY: usize = 3;
const BUILTIN_CHALLENGE_TTL: Duration = Duration::from_secs(10 * 60);
const BUILTIN_TOKEN_TTL: Duration = Duration::from_secs(20 * 60);
const BUILTIN_STORE_MAX_ENTRIES: usize = 10_000;

#[derive(Clone)]
pub struct CapVerifier {
    mode: Arc<CapMode>,
}

enum CapMode {
    BuiltIn(BuiltInCap),
    External(ConfiguredCap),
}

struct ConfiguredCap {
    api_endpoint: reqwest::Url,
    verify_endpoint: reqwest::Url,
    secret_key: String,
    http: reqwest::Client,
}

struct BuiltInCap {
    stores: Mutex<BuiltInStores>,
}

#[derive(Default)]
struct BuiltInStores {
    challenges: HashMap<String, Instant>,
    tokens: HashMap<[u8; 32], Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapVerifyError {
    Required,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInCapError {
    NotBuiltIn,
    Capacity,
}

#[derive(Debug, Serialize)]
pub struct BuiltInChallengeResponse {
    challenge: BuiltInChallengeParameters,
    token: String,
    expires: u64,
}

#[derive(Debug, Serialize)]
struct BuiltInChallengeParameters {
    c: usize,
    s: usize,
    d: usize,
}

#[derive(Debug, Deserialize)]
pub struct BuiltInRedeemRequest {
    pub token: String,
    pub solutions: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct BuiltInRedeemResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
}

#[derive(Serialize)]
struct SiteVerifyRequest<'a> {
    secret: &'a str,
    response: &'a str,
}

#[derive(Deserialize)]
struct SiteVerifyResponse {
    success: bool,
}

impl CapVerifier {
    pub fn builtin() -> Self {
        Self {
            mode: Arc::new(CapMode::BuiltIn(BuiltInCap {
                stores: Mutex::new(BuiltInStores::default()),
            })),
        }
    }

    pub fn from_env() -> Result<Self, String> {
        let endpoint = nonempty_env(CAP_API_ENDPOINT_ENV);
        let secret = nonempty_env(CAP_SECRET_KEY_ENV);
        match (endpoint, secret) {
            (None, None) => Ok(Self::builtin()),
            (Some(endpoint), Some(secret)) => Self::configured(&endpoint, secret),
            _ => Err(format!(
                "{CAP_API_ENDPOINT_ENV} and {CAP_SECRET_KEY_ENV} must be configured together"
            )),
        }
    }

    pub fn configured(api_endpoint: &str, secret_key: String) -> Result<Self, String> {
        let api_endpoint = normalize_api_endpoint(api_endpoint)?;
        let secret_key = secret_key.trim().to_string();
        if secret_key.is_empty() {
            return Err(format!("{CAP_SECRET_KEY_ENV} must not be empty"));
        }
        let verify_endpoint = api_endpoint
            .join("siteverify")
            .map_err(|error| format!("failed to construct Cap siteverify URL: {error}"))?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(VERIFY_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("failed to build Cap verification client: {error}"))?;
        Ok(Self {
            mode: Arc::new(CapMode::External(ConfiguredCap {
                api_endpoint,
                verify_endpoint,
                secret_key,
                http,
            })),
        })
    }

    pub fn public_api_endpoint(&self) -> &str {
        match self.mode.as_ref() {
            CapMode::BuiltIn(_) => BUILTIN_CAP_API_ENDPOINT,
            CapMode::External(configured) => configured.api_endpoint.as_str(),
        }
    }

    pub fn api_origin(&self) -> Option<String> {
        let CapMode::External(configured) = self.mode.as_ref() else {
            return None;
        };
        let origin = configured.api_endpoint.origin().ascii_serialization();
        (origin != "null").then_some(origin)
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self.mode.as_ref(), CapMode::BuiltIn(_))
    }

    pub async fn verify(&self, token: &str) -> Result<(), CapVerifyError> {
        let token = token.trim();
        if token.is_empty() || token.len() > CAPTCHA_TOKEN_MAX_BYTES {
            return Err(CapVerifyError::Required);
        }
        match self.mode.as_ref() {
            CapMode::BuiltIn(builtin) => builtin.verify_token(token),
            CapMode::External(configured) => verify_external(configured, token).await,
        }
    }

    pub fn create_builtin_challenge(&self) -> Result<BuiltInChallengeResponse, BuiltInCapError> {
        let CapMode::BuiltIn(builtin) = self.mode.as_ref() else {
            return Err(BuiltInCapError::NotBuiltIn);
        };
        builtin.create_challenge()
    }

    pub fn redeem_builtin_challenge(
        &self,
        request: &BuiltInRedeemRequest,
    ) -> Result<BuiltInRedeemResponse, BuiltInCapError> {
        let CapMode::BuiltIn(builtin) = self.mode.as_ref() else {
            return Err(BuiltInCapError::NotBuiltIn);
        };
        builtin.redeem(request)
    }
}

impl BuiltInCap {
    fn create_challenge(&self) -> Result<BuiltInChallengeResponse, BuiltInCapError> {
        let now = Instant::now();
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cleanup_expired(&mut stores, now);
        if stores.challenges.len() >= BUILTIN_STORE_MAX_ENTRIES {
            return Err(BuiltInCapError::Capacity);
        }

        let token = loop {
            let candidate = random_token();
            if !stores.challenges.contains_key(&candidate) {
                break candidate;
            }
        };
        stores
            .challenges
            .insert(token.clone(), now + BUILTIN_CHALLENGE_TTL);
        Ok(BuiltInChallengeResponse {
            challenge: BuiltInChallengeParameters {
                c: BUILTIN_CHALLENGE_COUNT,
                s: BUILTIN_CHALLENGE_SIZE,
                d: BUILTIN_CHALLENGE_DIFFICULTY,
            },
            token,
            expires: unix_time_millis(BUILTIN_CHALLENGE_TTL),
        })
    }

    fn redeem(
        &self,
        request: &BuiltInRedeemRequest,
    ) -> Result<BuiltInRedeemResponse, BuiltInCapError> {
        let now = Instant::now();
        {
            let mut stores = self
                .stores
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cleanup_expired(&mut stores, now);
            if !stores.challenges.contains_key(&request.token) {
                return Ok(BuiltInRedeemResponse::failed());
            }
        }

        let Some(solutions) = parse_solutions(&request.solutions) else {
            return Ok(BuiltInRedeemResponse::failed());
        };
        if !validate_solutions(&request.token, &solutions) {
            return Ok(BuiltInRedeemResponse::failed());
        }

        let auth_token = format!("{}:{}", random_token(), random_token());
        let auth_token_digest: [u8; 32] = Sha256::digest(auth_token.as_bytes()).into();
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cleanup_expired(&mut stores, now);
        if stores.tokens.len() >= BUILTIN_STORE_MAX_ENTRIES {
            return Err(BuiltInCapError::Capacity);
        }
        if stores.challenges.remove(&request.token).is_none() {
            return Ok(BuiltInRedeemResponse::failed());
        }
        stores
            .tokens
            .insert(auth_token_digest, now + BUILTIN_TOKEN_TTL);
        Ok(BuiltInRedeemResponse {
            success: true,
            token: Some(auth_token),
            expires: Some(unix_time_millis(BUILTIN_TOKEN_TTL)),
        })
    }

    fn verify_token(&self, token: &str) -> Result<(), CapVerifyError> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let now = Instant::now();
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cleanup_expired(&mut stores, now);
        stores
            .tokens
            .remove(&digest)
            .map(|_| ())
            .ok_or(CapVerifyError::Invalid)
    }
}

impl BuiltInRedeemResponse {
    fn failed() -> Self {
        Self {
            success: false,
            token: None,
            expires: None,
        }
    }
}

fn cleanup_expired(stores: &mut BuiltInStores, now: Instant) {
    stores.challenges.retain(|_, expires| *expires > now);
    stores.tokens.retain(|_, expires| *expires > now);
}

fn parse_solutions(values: &[Value]) -> Option<Vec<u64>> {
    if values.len() != BUILTIN_CHALLENGE_COUNT {
        return None;
    }
    values.iter().map(Value::as_u64).collect()
}

fn validate_solutions(token: &str, solutions: &[u64]) -> bool {
    let token_hash = fnv1a(token.as_bytes());
    solutions.iter().enumerate().all(|(index, solution)| {
        let index = (index + 1).to_string();
        let salt_seed = fnv1a_resume(token_hash, index.as_bytes());
        let target_seed = fnv1a_resume(salt_seed, b"d");
        let salt = prng_from_hash(salt_seed, BUILTIN_CHALLENGE_SIZE);
        let target = prng_from_hash(target_seed, BUILTIN_CHALLENGE_DIFFICULTY);
        let digest = Sha256::digest(format!("{salt}{solution}").as_bytes());
        digest_matches_hex_prefix(&digest, target.as_bytes())
    })
}

fn fnv1a(input: &[u8]) -> u32 {
    fnv1a_resume(2_166_136_261, input)
}

fn fnv1a_resume(mut hash: u32, input: &[u8]) -> u32 {
    for byte in input {
        hash ^= u32::from(*byte);
        hash = hash
            .wrapping_add(hash << 1)
            .wrapping_add(hash << 4)
            .wrapping_add(hash << 7)
            .wrapping_add(hash << 8)
            .wrapping_add(hash << 24);
    }
    hash
}

fn prng_from_hash(mut state: u32, length: usize) -> String {
    let mut result = String::with_capacity(length + 7);
    while result.len() < length {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        result.push_str(&format!("{state:08x}"));
    }
    result.truncate(length);
    result
}

fn digest_matches_hex_prefix(digest: &[u8], prefix: &[u8]) -> bool {
    prefix.iter().enumerate().all(|(index, expected)| {
        let byte = digest[index / 2];
        let actual = if index % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0f
        };
        actual == hex_nibble(*expected)
    })
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => u8::MAX,
    }
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn unix_time_millis(after: Duration) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .saturating_add(after)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

async fn verify_external(configured: &ConfiguredCap, token: &str) -> Result<(), CapVerifyError> {
    let response = configured
        .http
        .post(configured.verify_endpoint.clone())
        .json(&SiteVerifyRequest {
            secret: &configured.secret_key,
            response: token,
        })
        .send()
        .await
        .map_err(|_| CapVerifyError::Unavailable)?;
    if !response.status().is_success() {
        return Err(CapVerifyError::Unavailable);
    }
    let body = read_limited_body(response, VERIFY_RESPONSE_MAX_BYTES).await?;
    let verification: SiteVerifyResponse =
        serde_json::from_slice(&body).map_err(|_| CapVerifyError::Unavailable)?;
    if verification.success {
        Ok(())
    } else {
        Err(CapVerifyError::Invalid)
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_api_endpoint(raw: &str) -> Result<reqwest::Url, String> {
    let mut endpoint = reqwest::Url::parse(raw.trim())
        .map_err(|error| format!("{CAP_API_ENDPOINT_ENV} is not a valid URL: {error}"))?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(format!(
            "{CAP_API_ENDPOINT_ENV} must use the http or https scheme"
        ));
    }
    if endpoint.host().is_none() || !endpoint.username().is_empty() || endpoint.password().is_some()
    {
        return Err(format!(
            "{CAP_API_ENDPOINT_ENV} must contain a host and must not contain credentials"
        ));
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        return Err(format!(
            "{CAP_API_ENDPOINT_ENV} must not contain a query string or fragment"
        ));
    }
    if !endpoint.path().ends_with('/') {
        let normalized_path = format!("{}/", endpoint.path());
        endpoint.set_path(&normalized_path);
    }
    Ok(endpoint)
}

async fn read_limited_body(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<bytes::Bytes, CapVerifyError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(CapVerifyError::Unavailable);
    }
    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| CapVerifyError::Unavailable)?;
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(CapVerifyError::Unavailable);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

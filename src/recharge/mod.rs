//! Recharge system (`recharge-system.spec.md`): payment-adapter registry,
//! exact amount arithmetic, order storage, notification processing, and the
//! dashboard API handlers.

pub mod amount;
mod epay;
pub mod handlers;
pub mod store;
mod stripe;

use axum::http::{HeaderMap, Method, StatusCode};
use serde_json::Value;

pub use store::{NotifyOutcome, RechargeChannel, RechargeOrder};

/// RC-G4 environment variables, each falling back to its default on unset,
/// empty, malformed, zero, or negative values.
pub fn order_ttl_secs() -> u64 {
    parse_positive_env("MONOIZE_RECHARGE_ORDER_TTL_SECS", 3600)
}

pub fn tick_interval_secs() -> u64 {
    parse_positive_env("MONOIZE_RECHARGE_TICK_INTERVAL_SECS", 60)
}

pub fn max_pending_orders() -> u64 {
    parse_positive_env("MONOIZE_RECHARGE_MAX_PENDING_ORDERS", 10)
}

fn parse_positive_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// RC-G2 derived URLs handed to `create_payment`.
#[derive(Debug, Clone)]
pub struct PaymentUrls {
    pub notify_url: String,
    pub return_url: String,
    pub cancel_url: String,
}

impl PaymentUrls {
    pub fn derive(origin: &str, payment_channel_id: &str, order_id: &str) -> Self {
        Self {
            notify_url: format!("{origin}/api/pay/notify/{payment_channel_id}"),
            return_url: format!("{origin}/dashboard/wallet?order_id={order_id}"),
            cancel_url: format!("{origin}/dashboard/wallet?order_id={order_id}&canceled=1"),
        }
    }
}

/// RC-P2 item 1: version 1 defines exactly one initiation kind, `redirect`.
#[derive(Debug, Clone)]
pub struct PaymentInitiation {
    pub url: String,
    /// Persisted to `recharge_orders.provider_order_id` before the RC-O5
    /// response returns when the provider assigns an id at create time
    /// (RC-T1); `None` for EPay (RC-E1).
    pub provider_order_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyResult {
    Success,
    Failure,
    Expired,
}

#[derive(Debug, Clone)]
pub struct VerifiedNotification {
    pub order_id: String,
    pub provider_order_id: Option<String>,
    pub result: NotifyResult,
    pub paid_amount: Option<String>,
    pub paid_currency: Option<String>,
}

/// RC-P2 item 2 verification outcome. `Ignored` covers Stripe event types
/// outside RC-T3, which are acknowledged without touching order state.
pub enum Verification {
    Verified(VerifiedNotification),
    Ignored,
}

#[derive(Debug)]
pub struct SignatureError;

/// RC-P2 item 3 ack outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOutcome {
    Credited,
    Duplicate,
    FailedRecorded,
    UnknownOrder,
    SignatureError,
}

pub struct AckResponse {
    pub status: StatusCode,
    pub content_type: &'static str,
    pub body: String,
}

/// Compile-time payment-adapter registry (RC-P1..RC-P6).
#[async_trait::async_trait]
pub trait PaymentAdapter: Send + Sync {
    fn type_id(&self) -> &'static str;

    /// RC-P4: validate `currency` and return the fractional scale.
    fn currency_scale(&self, currency: &str) -> Result<u32, String>;

    /// RC-P5 secret field names inside `config_json`.
    fn secret_fields(&self) -> &'static [&'static str];

    /// RC-P5: validate a canonical config object. `require_secrets` is true on
    /// create (RC-P6: every secret field must be non-empty on POST).
    fn validate_config(&self, config: &Value, require_secrets: bool) -> Result<(), String>;

    /// RC-P3 capabilities.
    fn supports_query(&self) -> bool {
        false
    }
    fn supports_refund(&self) -> bool {
        false
    }

    async fn create_payment(
        &self,
        order: &RechargeOrder,
        config: &Value,
        urls: &PaymentUrls,
    ) -> Result<PaymentInitiation, String>;

    fn verify_notification(
        &self,
        method: &Method,
        headers: &HeaderMap,
        raw_body: &[u8],
        query: &str,
        config: &Value,
    ) -> Result<Verification, SignatureError>;

    fn ack(&self, outcome: AckOutcome) -> AckResponse;

    /// RC-R4: provider-side full refund; only called when
    /// `supports_refund() == true`.
    async fn refund(&self, _order: &RechargeOrder, _config: &Value) -> Result<(), String> {
        Err("refund not supported".to_string())
    }
}

static EPAY: epay::EpayAdapter = epay::EpayAdapter;
static STRIPE: stripe::StripeAdapter = stripe::StripeAdapter;

/// RC-P1: version 1 contains exactly `epay` and `stripe`.
pub fn adapter_for(type_id: &str) -> Option<&'static dyn PaymentAdapter> {
    match type_id {
        "epay" => Some(&EPAY),
        "stripe" => Some(&STRIPE),
        _ => None,
    }
}

/// Constant-time byte-string equality (RC-C1). The XOR fold inspects every
/// byte of equal-length inputs so timing does not leak the mismatch position.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Minimal `application/x-www-form-urlencoded` / query-string decoder used by
/// the EPay adapter (GET query and POST form bodies).
pub(crate) fn parse_form_urlencoded(raw: &str) -> Vec<(String, String)> {
    fn decode(component: &str) -> String {
        let plus_decoded = component.replace('+', " ");
        percent_encoding::percent_decode_str(&plus_decoded)
            .decode_utf8_lossy()
            .into_owned()
    }
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (decode(key), decode(value)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_parser_falls_back_on_invalid_values() {
        assert_eq!(parse_positive_env("MONOIZE_RECHARGE_TEST_UNSET", 3600), 3600);
        // SAFETY: test-only env mutation in a single-threaded test context.
        unsafe {
            std::env::set_var("MONOIZE_RECHARGE_TEST_A", "0");
            std::env::set_var("MONOIZE_RECHARGE_TEST_B", "-4");
            std::env::set_var("MONOIZE_RECHARGE_TEST_C", "120");
        }
        assert_eq!(parse_positive_env("MONOIZE_RECHARGE_TEST_A", 60), 60);
        assert_eq!(parse_positive_env("MONOIZE_RECHARGE_TEST_B", 60), 60);
        assert_eq!(parse_positive_env("MONOIZE_RECHARGE_TEST_C", 60), 120);
    }

    #[test]
    fn registry_contains_exactly_two_adapters() {
        assert!(adapter_for("epay").is_some());
        assert!(adapter_for("stripe").is_some());
        assert!(adapter_for("paypal").is_none());
    }

    #[test]
    fn constant_time_eq_compares_content() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn form_decoder_handles_percent_and_plus() {
        let pairs = parse_form_urlencoded("a=1&name=Monoize+Recharge&x=%E4%B8%AD");
        assert_eq!(pairs[0], ("a".to_string(), "1".to_string()));
        assert_eq!(pairs[1], ("name".to_string(), "Monoize Recharge".to_string()));
        assert_eq!(pairs[2], ("x".to_string(), "中".to_string()));
    }
}

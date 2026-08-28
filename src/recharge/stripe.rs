//! Stripe protocol binding (`recharge-system.spec.md` §2.2).

use super::{
    AckOutcome, AckResponse, NotifyResult, PaymentAdapter, PaymentInitiation, PaymentUrls,
    SignatureError, Verification, VerifiedNotification, constant_time_eq,
};
use crate::recharge::amount::format_minor_units;
use crate::recharge::store::RechargeOrder;
use axum::http::{HeaderMap, Method, StatusCode};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::sync::OnceLock;

pub struct StripeAdapter;

const STRIPE_API_BASE: &str = "https://api.stripe.com";
const SIGNATURE_TOLERANCE_SECS: i64 = 300;

/// RC-P4: Stripe zero-decimal currency set.
const ZERO_DECIMAL: [&str; 16] = [
    "BIF", "CLP", "DJF", "GNF", "JPY", "KMF", "KRW", "MGA", "PYG", "RWF", "UGX", "VND", "VUV",
    "XAF", "XOF", "XPF",
];

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

fn config_str<'a>(config: &'a Value, key: &str) -> &'a str {
    config.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `pay_amount` carries exactly `scale` fractional digits (RC-U4), so the
/// minor-unit integer is the digit string with the point removed.
fn minor_units_from_pay_amount(pay_amount: &str) -> String {
    let stripped: String = pay_amount.chars().filter(|c| *c != '.').collect();
    let trimmed = stripped.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn compute_signature(secret: &str, timestamp: &str, raw_body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    hex::encode(mac.finalize().into_bytes())
}

#[async_trait::async_trait]
impl PaymentAdapter for StripeAdapter {
    fn type_id(&self) -> &'static str {
        "stripe"
    }

    fn currency_scale(&self, currency: &str) -> Result<u32, String> {
        let valid = currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase());
        if !valid {
            return Err("stripe currency must be a 3-letter uppercase ISO 4217 code".to_string());
        }
        Ok(if ZERO_DECIMAL.contains(&currency) {
            0
        } else {
            2
        })
    }

    fn secret_fields(&self) -> &'static [&'static str] {
        &["secret_key", "webhook_secret"]
    }

    fn validate_config(&self, config: &Value, require_secrets: bool) -> Result<(), String> {
        let object = config
            .as_object()
            .ok_or_else(|| "config must be a JSON object".to_string())?;
        for key in object.keys() {
            if !matches!(key.as_str(), "secret_key" | "webhook_secret") {
                return Err(format!("unknown stripe config field {key}"));
            }
        }
        if require_secrets {
            if config_str(config, "secret_key").is_empty() {
                return Err("secret_key must be non-empty".to_string());
            }
            if config_str(config, "webhook_secret").is_empty() {
                return Err("webhook_secret must be non-empty".to_string());
            }
        }
        Ok(())
    }

    fn supports_refund(&self) -> bool {
        true
    }

    async fn create_payment(
        &self,
        order: &RechargeOrder,
        config: &Value,
        urls: &PaymentUrls,
    ) -> Result<PaymentInitiation, String> {
        let unit_amount = minor_units_from_pay_amount(&order.pay_amount);
        let form: Vec<(&str, String)> = vec![
            ("mode", "payment".to_string()),
            (
                "line_items[0][price_data][currency]",
                order.pay_currency.to_lowercase(),
            ),
            ("line_items[0][price_data][unit_amount]", unit_amount),
            (
                "line_items[0][price_data][product_data][name]",
                "Monoize Recharge".to_string(),
            ),
            ("line_items[0][quantity]", "1".to_string()),
            ("client_reference_id", order.id.clone()),
            ("metadata[order_id]", order.id.clone()),
            ("success_url", urls.return_url.clone()),
            ("cancel_url", urls.cancel_url.clone()),
        ];
        let response = http_client()
            .post(format!("{STRIPE_API_BASE}/v1/checkout/sessions"))
            .bearer_auth(config_str(config, "secret_key"))
            .form(&form)
            .send()
            .await
            .map_err(|error| format!("stripe request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "stripe checkout session creation failed with status {}",
                response.status()
            ));
        }
        let session: Value = response
            .json()
            .await
            .map_err(|error| format!("stripe response parse failed: {error}"))?;
        let url = session
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "stripe session has no url".to_string())?
            .to_string();
        let session_id = session
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "stripe session has no id".to_string())?
            .to_string();
        Ok(PaymentInitiation {
            url,
            provider_order_id: Some(session_id),
        })
    }

    fn verify_notification(
        &self,
        method: &Method,
        headers: &HeaderMap,
        raw_body: &[u8],
        _query: &str,
        config: &Value,
    ) -> Result<Verification, SignatureError> {
        // RC-T2: POST only.
        if *method != Method::POST {
            return Err(SignatureError);
        }
        let header = headers
            .get("stripe-signature")
            .and_then(|value| value.to_str().ok())
            .ok_or(SignatureError)?;
        let mut timestamp: Option<&str> = None;
        let mut v1_signatures: Vec<&str> = Vec::new();
        for part in header.split(',') {
            match part.trim().split_once('=') {
                Some(("t", value)) => timestamp = Some(value),
                Some(("v1", value)) => v1_signatures.push(value),
                _ => {}
            }
        }
        let timestamp = timestamp.ok_or(SignatureError)?;
        let timestamp_secs = timestamp.parse::<i64>().map_err(|_| SignatureError)?;
        let now = chrono::Utc::now().timestamp();
        if (now - timestamp_secs).abs() > SIGNATURE_TOLERANCE_SECS {
            return Err(SignatureError);
        }
        let expected = compute_signature(config_str(config, "webhook_secret"), timestamp, raw_body);
        let matched = v1_signatures
            .iter()
            .any(|candidate| constant_time_eq(expected.as_bytes(), candidate.as_bytes()));
        if !matched {
            return Err(SignatureError);
        }

        let event: Value = serde_json::from_slice(raw_body).map_err(|_| SignatureError)?;
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        let object = event
            .pointer("/data/object")
            .cloned()
            .unwrap_or(Value::Null);
        let order_id = object
            .get("client_reference_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let provider_order_id = object.get("id").and_then(Value::as_str).map(str::to_string);

        // RC-T3 event mapping; everything else is acknowledged and ignored.
        let verified = match event_type {
            "checkout.session.completed" | "checkout.session.async_payment_succeeded" => {
                if event_type == "checkout.session.completed"
                    && object.get("payment_status").and_then(Value::as_str) != Some("paid")
                {
                    return Ok(Verification::Ignored);
                }
                let currency = object
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_uppercase();
                let paid_amount = object
                    .get("amount_total")
                    .and_then(Value::as_i64)
                    .map(|total| {
                        let scale = self.currency_scale(&currency).unwrap_or(2);
                        format_minor_units(total as i128, scale)
                    });
                VerifiedNotification {
                    order_id,
                    provider_order_id,
                    result: NotifyResult::Success,
                    paid_amount,
                    paid_currency: Some(currency),
                }
            }
            "checkout.session.async_payment_failed" => VerifiedNotification {
                order_id,
                provider_order_id,
                result: NotifyResult::Failure,
                paid_amount: None,
                paid_currency: None,
            },
            "checkout.session.expired" => VerifiedNotification {
                order_id,
                provider_order_id,
                result: NotifyResult::Expired,
                paid_amount: None,
                paid_currency: None,
            },
            _ => return Ok(Verification::Ignored),
        };
        Ok(Verification::Verified(verified))
    }

    fn ack(&self, outcome: AckOutcome) -> AckResponse {
        // RC-T4 mapping.
        let (status, body) = match outcome {
            AckOutcome::SignatureError => {
                (StatusCode::BAD_REQUEST, r#"{"error":"invalid signature"}"#)
            }
            _ => (StatusCode::OK, r#"{"received":true}"#),
        };
        AckResponse {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }

    async fn refund(&self, order: &RechargeOrder, config: &Value) -> Result<(), String> {
        let session_id = order
            .provider_order_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "order has no stripe session id".to_string())?;
        let secret_key = config_str(config, "secret_key");
        let session_response = http_client()
            .get(format!(
                "{STRIPE_API_BASE}/v1/checkout/sessions/{session_id}"
            ))
            .bearer_auth(secret_key)
            .send()
            .await
            .map_err(|error| format!("stripe session lookup failed: {error}"))?;
        if !session_response.status().is_success() {
            return Err(format!(
                "stripe session lookup failed with status {}",
                session_response.status()
            ));
        }
        let session: Value = session_response
            .json()
            .await
            .map_err(|error| format!("stripe session parse failed: {error}"))?;
        let payment_intent = session
            .get("payment_intent")
            .and_then(Value::as_str)
            .ok_or_else(|| "stripe session has no payment_intent".to_string())?;
        let response = http_client()
            .post(format!("{STRIPE_API_BASE}/v1/refunds"))
            .bearer_auth(secret_key)
            .form(&[("payment_intent", payment_intent)])
            .send()
            .await
            .map_err(|error| format!("stripe refund failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "stripe refund failed with status {}",
                response.status()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_headers(secret: &str, timestamp: i64, body: &[u8]) -> HeaderMap {
        let signature = compute_signature(secret, &timestamp.to_string(), body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "stripe-signature",
            format!("t={timestamp},v1={signature}")
                .parse()
                .expect("header value"),
        );
        headers
    }

    fn completed_event(order_id: &str) -> Vec<u8> {
        serde_json::json!({
            "type": "checkout.session.completed",
            "data": { "object": {
                "id": "cs_test_1",
                "client_reference_id": order_id,
                "payment_status": "paid",
                "amount_total": 7300,
                "currency": "usd"
            }}
        })
        .to_string()
        .into_bytes()
    }

    /// Spec §15 T4: valid `v1` HMAC accepted; stale timestamp rejected; wrong
    /// secret rejected.
    #[test]
    fn signature_validation_matrix() {
        let adapter = StripeAdapter;
        let config = serde_json::json!({ "webhook_secret": "whsec_test" });
        let body = completed_event("a".repeat(32).as_str());
        let now = chrono::Utc::now().timestamp();

        let valid = adapter.verify_notification(
            &Method::POST,
            &signed_headers("whsec_test", now, &body),
            &body,
            "",
            &config,
        );
        match valid {
            Ok(Verification::Verified(notification)) => {
                assert_eq!(notification.result, NotifyResult::Success);
                assert_eq!(notification.paid_amount.as_deref(), Some("73.00"));
                assert_eq!(notification.paid_currency.as_deref(), Some("USD"));
            }
            _ => panic!("valid signature must verify"),
        }

        assert!(
            adapter
                .verify_notification(
                    &Method::POST,
                    &signed_headers("whsec_test", now - 301, &body),
                    &body,
                    "",
                    &config,
                )
                .is_err(),
            "stale timestamp must be rejected"
        );

        assert!(
            adapter
                .verify_notification(
                    &Method::POST,
                    &signed_headers("whsec_wrong", now, &body),
                    &body,
                    "",
                    &config,
                )
                .is_err(),
            "wrong secret must be rejected"
        );
    }

    #[test]
    fn non_checkout_events_are_ignored() {
        let adapter = StripeAdapter;
        let config = serde_json::json!({ "webhook_secret": "whsec_test" });
        let body = serde_json::json!({ "type": "invoice.paid", "data": { "object": {} } })
            .to_string()
            .into_bytes();
        let now = chrono::Utc::now().timestamp();
        match adapter.verify_notification(
            &Method::POST,
            &signed_headers("whsec_test", now, &body),
            &body,
            "",
            &config,
        ) {
            Ok(Verification::Ignored) => {}
            _ => panic!("non-checkout events must be acknowledged and ignored"),
        }
    }

    #[test]
    fn currency_scale_follows_zero_decimal_set() {
        let adapter = StripeAdapter;
        assert_eq!(adapter.currency_scale("USD").expect("valid"), 2);
        assert_eq!(adapter.currency_scale("JPY").expect("valid"), 0);
        assert!(adapter.currency_scale("usd").is_err());
        assert!(adapter.currency_scale("USDT").is_err());
    }
}

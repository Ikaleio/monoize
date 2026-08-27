//! EPay (易支付) protocol binding (`recharge-system.spec.md` §2.1).

use super::{
    AckOutcome, AckResponse, NotifyResult, PaymentAdapter, PaymentInitiation, PaymentUrls,
    SignatureError, Verification, VerifiedNotification, constant_time_eq, parse_form_urlencoded,
};
use crate::recharge::store::RechargeOrder;
use axum::http::{HeaderMap, Method, StatusCode};
use md5::{Digest, Md5};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

pub struct EpayAdapter;

/// RC-E2: lowercase hex `md5(join("&", sorted "k=v" pairs) + merchant_key)`
/// over every parameter except `sign`/`sign_type`, excluding empty values,
/// sorted bytewise ascending on the parameter name.
fn epay_sign(pairs: &[(String, String)], merchant_key: &str) -> String {
    let mut signable: Vec<&(String, String)> = pairs
        .iter()
        .filter(|(key, value)| key != "sign" && key != "sign_type" && !value.is_empty())
        .collect();
    signable.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let joined = signable
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut hasher = Md5::new();
    hasher.update(joined.as_bytes());
    hasher.update(merchant_key.as_bytes());
    hex::encode(hasher.finalize())
}

fn config_str<'a>(config: &'a Value, key: &str) -> &'a str {
    config.get(key).and_then(Value::as_str).unwrap_or("")
}

#[async_trait::async_trait]
impl PaymentAdapter for EpayAdapter {
    fn type_id(&self) -> &'static str {
        "epay"
    }

    fn currency_scale(&self, currency: &str) -> Result<u32, String> {
        if currency == "CNY" {
            Ok(2)
        } else {
            Err("epay currency must be CNY".to_string())
        }
    }

    fn secret_fields(&self) -> &'static [&'static str] {
        &["merchant_key"]
    }

    fn validate_config(&self, config: &Value, require_secrets: bool) -> Result<(), String> {
        let object = config
            .as_object()
            .ok_or_else(|| "config must be a JSON object".to_string())?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "gateway_url" | "merchant_id" | "merchant_key" | "pay_type"
            ) {
                return Err(format!("unknown epay config field {key}"));
            }
        }
        let gateway_url = config_str(config, "gateway_url");
        if !(gateway_url.starts_with("http://") || gateway_url.starts_with("https://"))
            || gateway_url.ends_with('/')
        {
            return Err("gateway_url must be an http(s) URL without a trailing slash".to_string());
        }
        if config_str(config, "merchant_id").is_empty() {
            return Err("merchant_id must be non-empty".to_string());
        }
        if require_secrets && config_str(config, "merchant_key").is_empty() {
            return Err("merchant_key must be non-empty".to_string());
        }
        if !config.get("pay_type").is_none_or(|value| value.is_string()) {
            return Err("pay_type must be a string".to_string());
        }
        Ok(())
    }

    async fn create_payment(
        &self,
        order: &RechargeOrder,
        config: &Value,
        urls: &PaymentUrls,
    ) -> Result<PaymentInitiation, String> {
        let gateway_url = config_str(config, "gateway_url");
        let merchant_key = config_str(config, "merchant_key");
        let pay_type = config_str(config, "pay_type");

        let mut pairs: Vec<(String, String)> = vec![
            ("pid".to_string(), config_str(config, "merchant_id").to_string()),
            ("out_trade_no".to_string(), order.id.clone()),
            ("notify_url".to_string(), urls.notify_url.clone()),
            ("return_url".to_string(), urls.return_url.clone()),
            ("name".to_string(), "Monoize Recharge".to_string()),
            ("money".to_string(), order.pay_amount.clone()),
        ];
        if !pay_type.is_empty() {
            pairs.push(("type".to_string(), pay_type.to_string()));
        }
        let sign = epay_sign(&pairs, merchant_key);
        pairs.push(("sign".to_string(), sign));
        pairs.push(("sign_type".to_string(), "MD5".to_string()));

        let query = pairs
            .iter()
            .map(|(key, value)| {
                format!(
                    "{key}={}",
                    utf8_percent_encode(value, NON_ALPHANUMERIC)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        Ok(PaymentInitiation {
            url: format!("{gateway_url}/submit.php?{query}"),
            provider_order_id: None,
        })
    }

    fn verify_notification(
        &self,
        method: &Method,
        _headers: &HeaderMap,
        raw_body: &[u8],
        query: &str,
        config: &Value,
    ) -> Result<Verification, SignatureError> {
        // RC-E3: GET carries parameters in the query, POST in the form body.
        let raw_params = if *method == Method::POST {
            String::from_utf8_lossy(raw_body).into_owned()
        } else {
            query.to_string()
        };
        let pairs = parse_form_urlencoded(&raw_params);
        let received_sign = pairs
            .iter()
            .find(|(key, _)| key == "sign")
            .map(|(_, value)| value.clone())
            .ok_or(SignatureError)?;
        let expected = epay_sign(&pairs, config_str(config, "merchant_key"));
        if !constant_time_eq(expected.as_bytes(), received_sign.to_lowercase().as_bytes()) {
            return Err(SignatureError);
        }
        let get = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let order_id = get("out_trade_no").ok_or(SignatureError)?;
        let result = if get("trade_status").as_deref() == Some("TRADE_SUCCESS") {
            NotifyResult::Success
        } else {
            NotifyResult::Failure
        };
        Ok(Verification::Verified(VerifiedNotification {
            order_id,
            provider_order_id: get("trade_no"),
            result,
            paid_amount: get("money"),
            paid_currency: Some("CNY".to_string()),
        }))
    }

    fn ack(&self, outcome: AckOutcome) -> AckResponse {
        // RC-E5 mapping.
        let (status, body) = match outcome {
            AckOutcome::Credited | AckOutcome::Duplicate | AckOutcome::FailedRecorded => {
                (StatusCode::OK, "success")
            }
            AckOutcome::UnknownOrder => (StatusCode::OK, "fail"),
            AckOutcome::SignatureError => (StatusCode::BAD_REQUEST, "fail"),
        };
        AckResponse {
            status,
            content_type: "text/plain",
            body: body.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_pairs(money: &str, key: &str) -> Vec<(String, String)> {
        let mut pairs = vec![
            ("pid".to_string(), "1001".to_string()),
            ("out_trade_no".to_string(), "a".repeat(32)),
            ("trade_no".to_string(), "provider-1".to_string()),
            ("trade_status".to_string(), "TRADE_SUCCESS".to_string()),
            ("money".to_string(), money.to_string()),
        ];
        let sign = epay_sign(&pairs, key);
        pairs.push(("sign".to_string(), sign));
        pairs.push(("sign_type".to_string(), "MD5".to_string()));
        pairs
    }

    fn to_query(pairs: &[(String, String)]) -> String {
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={}", utf8_percent_encode(v, NON_ALPHANUMERIC)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Spec §15 T3: RC-E2 sign round-trip; a mutated `money` fails.
    #[test]
    fn sign_round_trip_and_tamper_detection() {
        let config = serde_json::json!({ "merchant_key": "secret-key" });
        let adapter = EpayAdapter;
        let query = to_query(&signed_pairs("73.00", "secret-key"));
        let verified = adapter
            .verify_notification(
                &Method::GET,
                &HeaderMap::new(),
                b"",
                &query,
                &config,
            )
            .ok();
        match verified {
            Some(Verification::Verified(notification)) => {
                assert_eq!(notification.result, NotifyResult::Success);
                assert_eq!(notification.paid_amount.as_deref(), Some("73.00"));
            }
            _ => panic!("valid signature must verify"),
        }

        let tampered = to_query(&{
            let mut pairs = signed_pairs("73.00", "secret-key");
            for pair in &mut pairs {
                if pair.0 == "money" {
                    pair.1 = "1.00".to_string();
                }
            }
            pairs
        });
        assert!(
            adapter
                .verify_notification(&Method::GET, &HeaderMap::new(), b"", &tampered, &config)
                .is_err()
        );
    }

    #[test]
    fn sign_excludes_empty_values_and_sorts_bytewise() {
        let pairs = vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
            ("empty".to_string(), String::new()),
        ];
        let mut hasher = Md5::new();
        hasher.update(b"a=1&b=2");
        hasher.update(b"key");
        assert_eq!(epay_sign(&pairs, "key"), hex::encode(hasher.finalize()));
    }
}

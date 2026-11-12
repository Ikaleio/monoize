use crate::exact_decimal::Multiplier;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct RequestLogNameSnapshots {
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub provider_name: Option<String>,
    pub channel_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InsertRequestLog {
    pub request_id: Option<String>,
    pub user_id: String,
    pub api_key_id: Option<String>,
    pub model: String,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub upstream_response_model: Option<String>,
    pub channel_id: Option<String>,
    pub names: RequestLogNameSnapshots,
    pub is_stream: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub tool_prompt_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub accepted_prediction_tokens: Option<u64>,
    pub rejected_prediction_tokens: Option<u64>,
    pub provider_multiplier: Option<Multiplier>,
    pub charge_nano_usd: Option<i128>,
    pub status: String,
    pub usage_breakdown_json: Option<Value>,
    pub billing_breakdown_json: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_http_status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub request_ip: Option<String>,
    pub reasoning_effort: Option<String>,
    pub tried_providers_json: Option<Value>,
    pub request_kind: Option<String>,
    pub effective_provider_type: Option<String>,
    pub affinity_hit: Option<bool>,
    pub affinity_key_hash: Option<String>,
    pub affinity_target: Option<String>,
    pub session_affinity_value: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub const REQUEST_LOG_STATUS_PENDING: &str = "pending";
pub const REQUEST_LOG_STATUS_SUCCESS: &str = "success";
pub const REQUEST_LOG_STATUS_ERROR: &str = "error";
pub const REQUEST_LOG_STATUS_CLIENT_GONE: &str = "client_gone";

#[derive(Debug, Serialize)]
pub struct RequestLogProvider {
    pub id: Option<String>,
    pub name: Option<String>,
    pub multiplier: Option<Multiplier>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogChannel {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogAffinity {
    pub hit: Option<bool>,
    pub key_hash: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogUser {
    pub id: String,
    pub username: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogApiKey {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogTokens {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_creation: Option<i64>,
    pub tool_prompt: Option<i64>,
    pub reasoning: Option<i64>,
    pub accepted_prediction: Option<i64>,
    pub rejected_prediction: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogTiming {
    pub duration_ms: Option<i64>,
    pub ttfb_ms: Option<i64>,
    #[serde(rename = "durationMs")]
    pub duration_ms_alias: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub latency_ms: Option<i64>,
    #[serde(rename = "ttfbMs")]
    pub ttfb_ms_alias: Option<i64>,
    pub first_token_ms: Option<i64>,
    #[serde(rename = "firstTokenMs")]
    pub first_token_ms_alias: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogBilling {
    pub charge_nano_usd: Option<String>,
    pub breakdown: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogError {
    pub code: Option<String>,
    pub message: Option<String>,
    pub http_status: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RequestLogRow {
    pub id: String,
    pub request_id: Option<String>,
    pub created_at: String,
    pub status: String,
    pub is_stream: bool,
    pub model: String,
    pub upstream_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_response_model: Option<String>,
    pub effective_provider_type: Option<String>,
    pub request_kind: Option<String>,
    pub reasoning_effort: Option<String>,
    pub request_ip: Option<String>,
    pub tried_providers: Option<Value>,
    pub session_affinity_value: Option<String>,
    pub provider: RequestLogProvider,
    pub channel: RequestLogChannel,
    pub affinity: RequestLogAffinity,
    pub user: RequestLogUser,
    pub api_key: RequestLogApiKey,
    pub tokens: RequestLogTokens,
    pub timing: RequestLogTiming,
    pub billing: RequestLogBilling,
    pub usage: Option<Value>,
    pub error: RequestLogError,
    /// RCV-L1/RCV-L2 (`request-capture-viewer.spec.md`): true iff a
    /// `request_capture_records` row matches this row's `(request_id, user_id)`.
    pub has_capture: bool,
}

impl RequestLogRow {
    /// SAN-14 (`spec/upstream-error-sanitization.spec.md`): replace the
    /// admin-tier error detail with its `MASK`ed form before serializing this
    /// row for a non-admin dashboard viewer. Operates on the in-memory copy
    /// only; the stored row is never modified.
    pub fn mask_error_detail_for_non_admin(&mut self) {
        if let Some(message) = self.error.message.as_deref() {
            self.error.message = Some(crate::error_sanitize::mask_sensitive_text(message));
        }
        let Some(Value::Array(items)) = self.tried_providers.as_mut() else {
            return;
        };
        for item in items {
            let Some(obj) = item.as_object_mut() else {
                continue;
            };
            if let Some(error_text) = obj.get("error").and_then(Value::as_str) {
                let masked = crate::error_sanitize::mask_sensitive_text(error_text);
                obj.insert("error".to_string(), Value::String(masked));
            }
        }
    }
    pub fn hide_upstream_response_model(&mut self) {
        self.upstream_response_model = None;
    }
}

impl InsertRequestLog {
    pub fn to_request_log_row(&self) -> RequestLogRow {
        RequestLogRow {
            id: self.request_id.clone().unwrap_or_default(),
            request_id: self.request_id.clone(),
            created_at: self.created_at.to_rfc3339(),
            status: self.status.clone(),
            is_stream: self.is_stream,
            model: self.model.clone(),
            upstream_model: self.upstream_model.clone(),
            upstream_response_model: self.upstream_response_model.clone(),
            effective_provider_type: self.effective_provider_type.clone(),
            request_kind: self.request_kind.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            request_ip: self.request_ip.clone(),
            tried_providers: self.tried_providers_json.clone(),
            session_affinity_value: self.session_affinity_value.clone(),
            // RCV-L3: SSE snapshots are emitted before capture persistence
            // completes, so live rows always report no capture.
            has_capture: false,
            provider: RequestLogProvider {
                id: self.provider_id.clone(),
                name: self.names.provider_name.clone(),
                multiplier: self.provider_multiplier,
            },
            channel: RequestLogChannel {
                id: self.channel_id.clone(),
                name: self.names.channel_name.clone(),
            },
            affinity: RequestLogAffinity {
                hit: self.affinity_hit,
                key_hash: self.affinity_key_hash.clone(),
                target: self.affinity_target.clone(),
            },
            user: RequestLogUser {
                id: self.user_id.clone(),
                username: self.names.username.clone(),
            },
            api_key: RequestLogApiKey {
                id: self.api_key_id.clone(),
                name: self.names.api_key_name.clone(),
            },
            tokens: RequestLogTokens {
                input: self.input_tokens.and_then(|v| i64::try_from(v).ok()),
                output: self.output_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_read: self.cache_read_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_creation: self
                    .cache_creation_tokens
                    .and_then(|v| i64::try_from(v).ok()),
                tool_prompt: self.tool_prompt_tokens.and_then(|v| i64::try_from(v).ok()),
                reasoning: self.reasoning_tokens.and_then(|v| i64::try_from(v).ok()),
                accepted_prediction: self
                    .accepted_prediction_tokens
                    .and_then(|v| i64::try_from(v).ok()),
                rejected_prediction: self
                    .rejected_prediction_tokens
                    .and_then(|v| i64::try_from(v).ok()),
            },
            timing: RequestLogTiming {
                duration_ms: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                ttfb_ms: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                duration_ms_alias: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                elapsed_ms: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                latency_ms: self.duration_ms.and_then(|v| i64::try_from(v).ok()),
                ttfb_ms_alias: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                first_token_ms: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
                first_token_ms_alias: self.ttfb_ms.and_then(|v| i64::try_from(v).ok()),
            },
            billing: RequestLogBilling {
                charge_nano_usd: self.charge_nano_usd.map(|v| v.to_string()),
                breakdown: self.billing_breakdown_json.clone(),
            },
            usage: self.usage_breakdown_json.clone(),
            error: RequestLogError {
                code: self.error_code.clone(),
                message: self.error_message.clone(),
                http_status: self.error_http_status.map(i64::from),
            },
        }
    }
}

pub struct AnalyticsModelBucketRow {
    pub bucket_idx: i64,
    pub model: String,
    pub cost_nano: i128,
    pub call_count: i64,
    pub token_count: i64,
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
}

pub struct AnalyticsProviderBucketRow {
    pub bucket_idx: i64,
    pub provider_label: String,
    pub call_count: i64,
}

pub struct DashboardAnalyticsRaw {
    pub model_buckets: Vec<AnalyticsModelBucketRow>,
    pub provider_buckets: Vec<AnalyticsProviderBucketRow>,
    pub total_cost_nano_usd: i128,
    pub total_calls: i64,
    pub today_cost_nano_usd: i128,
    pub today_calls: i64,
}

pub struct PerformanceHourBucketRow {
    pub hour_idx: i64,
    pub finished_count: i64,
    pub success_count: i64,
}

pub struct PerformanceTargetRaw {
    pub hour_buckets: Vec<PerformanceHourBucketRow>,
    pub avg_ttft_ms: Option<f64>,
    pub avg_tps: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct UserTodayUsage {
    pub user_id: String,
    pub today_calls: i64,
    pub today_cost_nano_usd: i128,
}

#[derive(Debug, Clone)]
pub struct UserUsageRankingRow {
    pub user_id: String,
    pub username: Option<String>,
    pub call_count: i64,
    pub cost_nano_usd: i128,
}

#[derive(Debug, Clone)]
pub struct ChannelWindowUsage {
    pub channel_id: String,
    pub window_calls: i64,
    pub window_cost_nano_usd: i128,
}

/// Live-usage aggregate window (`user-live-usage.spec.md` LU-4).
pub const LIVE_USAGE_WINDOW_SECONDS: i64 = 60;

/// Rolling 60-second per-user request-log aggregate
/// (`user-live-usage.spec.md` LU-6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UserLiveUsage {
    pub rpm: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderLiveUsage {
    pub rpm: i64,
    pub tpm: i64,
}

use super::config::http_body_max_bytes;
use super::*;

async fn canonical_request_id_middleware(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let canonical = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| axum::http::HeaderValue::from_str(value).ok())
        .unwrap_or_else(|| {
            axum::http::HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
                .expect("UUID request id is a valid header value")
        });
    request.headers_mut().insert(
        axum::http::header::HeaderName::from_static("x-request-id"),
        canonical.clone(),
    );
    request.extensions_mut().insert(RequestId::new(canonical));
    next.run(request).await
}

/// D1: replica nodes answer every non-API path with this error instead of mounting
/// the dashboard or the SPA.
async fn replica_disabled_fallback() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(json!({
            "error": {
                "code": "replica_dashboard_disabled",
                "message": "dashboard and frontend are served by the primary node"
            }
        })),
    )
        .into_response()
}

pub fn build_app(state: AppState) -> Router {
    let metrics_path = state.runtime.metrics_path.clone();
    let trusted_proxies = state.trusted_proxies.clone();
    let http_body_max_bytes = http_body_max_bytes();
    let is_replica = state.node.is_replica();
    let root_api_router = build_root_api_router(&metrics_path);
    let dashboard_api_router = build_dashboard_api_router();
    let csp = ContentSecurityPolicy::new(state.cap_verifier.api_origin());

    let mut app = Router::<AppState>::new()
        .merge(root_api_router.clone())
        .merge(build_balance_compatibility_router());
    if is_replica {
        // D1/D2: API-only surface; /v1/** and /metrics stay local.
        app = app.fallback(replica_disabled_fallback);
    } else {
        let api_router = root_api_router.clone().merge(dashboard_api_router);
        app = app.nest("/api", api_router);
        if state.metering_token_digest.is_some() {
            // PRP6/I1: ingest mounted only when a replica token is configured.
            app = app.route(
                crate::replica::metering::METERING_INGEST_PATH,
                post(crate::replica::metering::ingest_metering_handler),
            );
        }
        app = app.fallback(crate::frontend::frontend_fallback);
    }
    app.with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            trusted_proxies,
            crate::client_ip::canonical_client_ip_middleware,
        ))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(http_body_max_bytes))
        .layer(axum::middleware::from_fn(
            crate::handlers::native_body_limit_middleware,
        ))
        .layer(PropagateRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
        ))
        .layer(axum::middleware::from_fn(canonical_request_id_middleware))
        .layer(SetRequestIdLayer::new(
            axum::http::header::HeaderName::from_static("x-request-id"),
            MakeRequestUuid,
        ))
        .layer(TraceLayer::new_for_http())
        // Security headers
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("x-content-type-options"),
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::HeaderName::from_static("x-frame-options"),
            axum::http::HeaderValue::from_static("DENY"),
        ))
        .layer(axum::middleware::from_fn_with_state(
            csp,
            content_security_policy_middleware,
        ))
}

#[derive(Clone)]
struct ContentSecurityPolicy {
    connect_src: String,
}

impl ContentSecurityPolicy {
    fn new(cap_origin: Option<String>) -> Self {
        let connect_src = match cap_origin {
            Some(origin) => format!("'self' {origin}"),
            None => "'self'".to_string(),
        };
        Self { connect_src }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct CspNonce(pub String);

async fn content_security_policy_middleware(
    axum::extract::State(config): axum::extract::State<ContentSecurityPolicy>,
    mut request: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    request.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut response = next.run(request).await;
    let policy = format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval' 'nonce-{nonce}'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fontsapi.zeoseven.com; img-src * data:; connect-src {}; font-src 'self' https://fonts.gstatic.com https://fontsapi.zeoseven.com; worker-src 'self' blob:; frame-ancestors 'none'",
        config.connect_src
    );
    if let Ok(value) = header::HeaderValue::from_str(&policy) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    response
}

fn build_v1_router() -> Router<AppState> {
    Router::new()
        .route("/v1/models", get(crate::handlers::list_models))
        .route(
            "/v1/responses",
            get(crate::handlers::responses_websocket).post(crate::handlers::create_response),
        )
        .route(
            "/v1/codex/responses",
            get(crate::handlers::responses_websocket).post(crate::handlers::create_response),
        )
        .route(
            "/v1/responses/compact",
            post(crate::handlers::compact_response),
        )
        .route(
            "/v1/chat/completions",
            post(crate::handlers::create_chat_completions),
        )
        .route("/v1/embeddings", post(crate::handlers::create_embeddings))
        .route("/v1/systemone", post(crate::handlers::create_system_one))
        .route("/v1/decisions", post(crate::handlers::create_decisions))
        .route("/v1/messages", post(crate::handlers::create_messages))
        .route(
            "/v1/messages/count_tokens",
            post(crate::handlers::count_messages_tokens),
        )
        .route(
            "/v1/images/generations",
            post(crate::handlers::image_api::create_image_generation),
        )
        .route(
            "/v1/images/edits",
            post(crate::handlers::image_api::create_image_edit),
        )
        .layer(CorsLayer::very_permissive())
}

fn build_root_api_router(metrics_path: &str) -> Router<AppState> {
    build_v1_router()
        .route(metrics_path, get(crate::dashboard_handlers::get_metrics))
        .route(
            "/presets/providers",
            get(crate::dashboard_handlers::get_provider_presets),
        )
        .route(
            "/presets/apikeys",
            get(crate::dashboard_handlers::get_apikey_presets),
        )
}

fn build_balance_compatibility_router() -> Router<AppState> {
    Router::new()
        .route("/api/codex/usage", get(crate::handlers::codex_usage))
        .route("/user/balance", get(crate::handlers::deepseek_user_balance))
        .layer(CorsLayer::very_permissive())
}

fn build_dashboard_api_router() -> Router<AppState> {
    let router = Router::new();
    let router = dashboard_account_routes(router);
    let router = dashboard_configuration_routes(router);
    let router = dashboard_model_routes(router);
    let router = dashboard_operations_routes(router);
    dashboard_payment_routes(router)
}

fn dashboard_account_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(
            "/dashboard/auth/register",
            post(crate::dashboard_handlers::register),
        )
        .route(
            "/dashboard/auth/login",
            post(crate::dashboard_handlers::login),
        )
        .route(
            "/dashboard/captcha/challenge",
            post(crate::dashboard_handlers::create_captcha_challenge),
        )
        .route(
            "/dashboard/captcha/redeem",
            post(crate::dashboard_handlers::redeem_captcha_challenge),
        )
        .route(
            "/dashboard/auth/logout",
            post(crate::dashboard_handlers::logout),
        )
        .route("/dashboard/auth/me", get(crate::dashboard_handlers::get_me))
        .route(
            "/dashboard/auth/me",
            put(crate::dashboard_handlers::update_me),
        )
        .route(
            "/dashboard/auth/password",
            put(crate::dashboard_handlers::change_password),
        )
        .route(
            "/dashboard/users",
            get(crate::dashboard_handlers::list_users),
        )
        .route(
            "/dashboard/users",
            post(crate::dashboard_handlers::create_user),
        )
        .route(
            "/dashboard/users/{user_id}",
            get(crate::dashboard_handlers::get_user),
        )
        .route(
            "/dashboard/users/{user_id}",
            put(crate::dashboard_handlers::update_user),
        )
        .route(
            "/dashboard/users/{user_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_user),
        )
        .route(
            "/dashboard/users/{user_id}/api-keys",
            get(crate::dashboard_handlers::list_user_api_keys_admin),
        )
        .route(
            "/dashboard/users/{user_id}/billing-plan-subscription",
            get(crate::dashboard_handlers::get_user_billing_plan_subscription)
                .put(crate::dashboard_handlers::assign_user_billing_plan_subscription)
                .delete(crate::dashboard_handlers::revoke_user_billing_plan_subscription),
        )
        .route(
            "/dashboard/billing-plans",
            get(crate::dashboard_handlers::list_billing_plans)
                .post(crate::dashboard_handlers::create_billing_plan),
        )
        .route(
            "/dashboard/billing-plans/marketplace",
            get(crate::dashboard_handlers::list_billing_plan_marketplace),
        )
        .route(
            "/dashboard/billing-plan-subscription",
            get(crate::dashboard_handlers::get_billing_plan_subscription),
        )
}

fn dashboard_configuration_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(
            "/dashboard/billing-plans/{plan_id}",
            put(crate::dashboard_handlers::update_billing_plan)
                .delete(crate::dashboard_handlers::delete_billing_plan),
        )
        .route(
            "/dashboard/billing-plans/{plan_id}/purchase",
            post(crate::dashboard_handlers::purchase_billing_plan),
        )
        .route(
            "/dashboard/tokens",
            get(crate::dashboard_handlers::list_my_api_keys),
        )
        .route(
            "/dashboard/tokens",
            post(crate::dashboard_handlers::create_api_key),
        )
        .route(
            "/dashboard/tokens/batch-delete",
            post(crate::dashboard_handlers::batch_delete_api_keys),
        )
        .route(
            "/dashboard/tokens/{key_id}",
            get(crate::dashboard_handlers::get_api_key),
        )
        .route(
            "/dashboard/tokens/{key_id}",
            put(crate::dashboard_handlers::update_api_key),
        )
        .route(
            "/dashboard/tokens/{key_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_api_key),
        )
        .route(
            "/dashboard/tokens/{key_id}/transfer",
            post(crate::dashboard_handlers::transfer_to_sub_account),
        )
        .route(
            "/dashboard/settings",
            get(crate::dashboard_handlers::get_settings),
        )
        .route(
            "/dashboard/settings",
            put(crate::dashboard_handlers::update_settings),
        )
        .route(
            "/dashboard/settings/public",
            get(crate::dashboard_handlers::get_public_settings),
        )
        .route(
            "/dashboard/stats",
            get(crate::dashboard_handlers::get_dashboard_stats),
        )
        .route(
            "/dashboard/config",
            get(crate::dashboard_handlers::get_config_overview),
        )
        .route(
            "/dashboard/groups",
            get(crate::dashboard_handlers::list_dashboard_groups),
        )
        .route(
            "/dashboard/groups",
            post(crate::dashboard_handlers::create_group),
        )
        .route(
            "/dashboard/groups/reorder",
            post(crate::dashboard_handlers::reorder_groups),
        )
        .route(
            "/dashboard/groups/{group_id}",
            put(crate::dashboard_handlers::update_group),
        )
}

fn dashboard_model_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(
            "/dashboard/groups/{group_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_group),
        )
        .route(
            "/dashboard/providers",
            get(crate::dashboard_handlers::list_providers),
        )
        .route(
            "/dashboard/providers",
            post(crate::dashboard_handlers::create_provider),
        )
        .route(
            "/dashboard/providers/reorder",
            post(crate::dashboard_handlers::reorder_providers),
        )
        .route(
            "/dashboard/providers/{provider_id}",
            get(crate::dashboard_handlers::get_provider),
        )
        .route(
            "/dashboard/providers/{provider_id}",
            put(crate::dashboard_handlers::update_provider),
        )
        .route(
            "/dashboard/providers/{provider_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_provider),
        )
        .route(
            "/dashboard/transforms/registry",
            get(crate::dashboard_handlers::get_transform_registry),
        )
        .route(
            "/dashboard/custom-transforms",
            get(crate::dashboard_handlers::list_custom_transforms)
                .post(crate::dashboard_handlers::create_custom_transform),
        )
        .route(
            "/dashboard/custom-transforms/references",
            get(crate::dashboard_handlers::list_custom_transform_references),
        )
        .route(
            "/dashboard/custom-transforms/{id}",
            put(crate::dashboard_handlers::update_custom_transform)
                .delete(crate::dashboard_handlers::delete_custom_transform),
        )
        // Model registry API routes
        .route(
            "/dashboard/models",
            get(crate::dashboard_handlers::list_models),
        )
        .route(
            "/dashboard/models",
            post(crate::dashboard_handlers::create_model),
        )
        .route(
            "/dashboard/models/{model_id}",
            get(crate::dashboard_handlers::get_model),
        )
        .route(
            "/dashboard/models/{model_id}",
            put(crate::dashboard_handlers::update_model),
        )
        .route(
            "/dashboard/models/{model_id}",
            axum::routing::delete(crate::dashboard_handlers::delete_model),
        )
        .route(
            "/dashboard/model-metadata",
            get(crate::dashboard_handlers::list_model_metadata),
        )
        .route(
            "/dashboard/marketplace/models",
            get(crate::dashboard_handlers::list_marketplace_models),
        )
}

fn dashboard_operations_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(
            "/dashboard/model-metadata/sync/models-dev",
            post(crate::dashboard_handlers::sync_model_metadata_models_dev),
        )
        .route(
            "/dashboard/model-metadata/{*model_id}",
            get(crate::dashboard_handlers::get_model_metadata)
                .put(crate::dashboard_handlers::upsert_model_metadata)
                .delete(crate::dashboard_handlers::delete_model_metadata),
        )
        .route(
            "/dashboard/model-prices",
            get(crate::dashboard_handlers::list_model_prices),
        )
        .route(
            "/dashboard/model-prices/unpriced",
            get(crate::dashboard_handlers::list_unpriced_models),
        )
        .route(
            "/dashboard/price-sync/runs",
            get(crate::dashboard_handlers::list_price_sync_runs),
        )
        .route(
            "/dashboard/price-sync/{source}/preview",
            post(crate::dashboard_handlers::preview_price_sync),
        )
        .route(
            "/dashboard/price-sync/{source}/apply",
            post(crate::dashboard_handlers::apply_price_sync),
        )
        .route(
            "/dashboard/model-prices/{*model_id}",
            put(crate::dashboard_handlers::upsert_model_price)
                .delete(crate::dashboard_handlers::delete_model_price),
        )
        .route(
            "/dashboard/providers/{provider_id}/channels/{channel_id}/test",
            post(crate::dashboard_handlers::test_channel),
        )
        .route(
            "/dashboard/providers/{provider_id}/channels/{channel_id}/enabled",
            put(crate::dashboard_handlers::set_channel_enabled),
        )
        .route(
            "/dashboard/fetch-channel-models",
            post(crate::dashboard_handlers::fetch_channel_models),
        )
        .route(
            "/dashboard/probe-channel-websocket",
            post(crate::dashboard_handlers::probe_channel_websocket),
        )
        .route(
            "/dashboard/request-logs/stream",
            get(crate::dashboard_handlers::stream_request_logs),
        )
        .route(
            "/dashboard/request-logs",
            get(crate::dashboard_handlers::list_my_request_logs),
        )
        .route(
            "/dashboard/request-captures/{request_id}",
            get(crate::dashboard_handlers::get_request_capture),
        )
        .route(
            "/dashboard/analytics",
            get(crate::dashboard_handlers::get_dashboard_analytics),
        )
        .route(
            "/dashboard/performance",
            get(crate::dashboard_handlers::get_dashboard_performance),
        )
        .route(
            "/dashboard/me/live-usage",
            get(crate::dashboard_handlers::get_my_live_usage),
        )
}

fn dashboard_payment_routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route(
            "/dashboard/admin/overview",
            get(crate::dashboard_handlers::get_admin_overview),
        )
        .route(
            "/dashboard/recharge/channels",
            get(crate::recharge::handlers::list_recharge_channels),
        )
        .route(
            "/dashboard/recharge/orders",
            get(crate::recharge::handlers::list_recharge_orders)
                .post(crate::recharge::handlers::create_recharge_order),
        )
        .route(
            "/dashboard/recharge/orders/{order_id}",
            get(crate::recharge::handlers::get_recharge_order),
        )
        .route(
            "/dashboard/recharge/orders/{order_id}/refund",
            post(crate::recharge::handlers::refund_recharge_order),
        )
        .route(
            "/dashboard/ledger",
            get(crate::recharge::handlers::list_ledger),
        )
        .route(
            "/dashboard/payment-channels",
            get(crate::recharge::handlers::list_payment_channels)
                .post(crate::recharge::handlers::create_payment_channel),
        )
        .route(
            "/dashboard/payment-channels/{channel_id}",
            put(crate::recharge::handlers::update_payment_channel)
                .delete(crate::recharge::handlers::delete_payment_channel),
        )
        // RC-N1: outside session auth; adapter signatures are the only
        // authenticity source. Mounted here so replicas never expose it.
        .route(
            "/pay/notify/{payment_channel_id}",
            get(crate::recharge::handlers::pay_notify).post(crate::recharge::handlers::pay_notify),
        )
}

use crate::app::AppState;
use crate::dashboard_handlers::session_helpers::get_current_user;
use crate::error::{AppError, AppResult};
use crate::users::parse_groups_json;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
pub struct DashboardGroupsResponse {
    pub groups: Vec<String>,
}

fn aggregate_group_labels(raw_group_arrays: impl IntoIterator<Item = String>) -> Vec<String> {
    raw_group_arrays
        .into_iter()
        .flat_map(|raw| parse_groups_json(&raw).into_iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub async fn list_dashboard_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<DashboardGroupsResponse>> {
    get_current_user(&headers, &state).await?;

    let channel_groups = state
        .monoize_store
        .list_all_channel_groups_json()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let user_groups = state
        .user_store
        .list_all_user_allowed_groups_json()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    let api_key_groups = state
        .user_store
        .list_all_api_key_allowed_groups_json()
        .await
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;

    let groups = aggregate_group_labels(
        channel_groups
            .into_iter()
            .chain(user_groups)
            .chain(api_key_groups),
    );

    Ok(Json(DashboardGroupsResponse { groups }))
}

#[cfg(test)]
mod tests {
    use super::{DashboardGroupsResponse, aggregate_group_labels, list_dashboard_groups};
    use crate::app::{RuntimeConfig, load_state_with_runtime};
    use crate::users::UserRole;
    use axum::Json;
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue};
    use sea_orm::ConnectionTrait;

    #[test]
    fn aggregate_group_labels_is_tolerant_unique_and_sorted() {
        let groups = aggregate_group_labels(vec![
            r#"[" Beta ","alpha","ALPHA",""]"#.to_string(),
            "not-json".to_string(),
            "   ".to_string(),
            r#"["gamma","beta"]"#.to_string(),
        ]);

        assert_eq!(
            groups,
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
    }

    #[tokio::test]
    async fn dashboard_groups_endpoint_returns_aggregated_labels_for_authenticated_user() {
        let state = load_state_with_runtime(RuntimeConfig {
            listen: "127.0.0.1:0".to_string(),
            metrics_path: "/metrics".to_string(),
            database_dsn: "sqlite::memory:".to_string(),
        })
        .await
        .expect("state loads");

        let user = state
            .user_store
            .create_user(
                "dashboard_reader",
                "password123",
                UserRole::User,
                &[" Team-A ".to_string(), "".to_string()],
            )
            .await
            .expect("user created");
        let session = state
            .user_store
            .create_session(&user.id, 7)
            .await
            .expect("session created");
        let api_key_owner = state
            .user_store
            .create_user("api_owner", "password123", UserRole::User, &[])
            .await
            .expect("api owner created");
        state
            .user_store
            .create_api_key_extended(
                &api_key_owner.id,
                crate::users::CreateApiKeyInput {
                    name: "reader key".to_string(),
                    expires_in_days: None,
                    quota: None,
                    quota_unlimited: true,
                    model_limits_enabled: false,
                    model_limits: Vec::new(),
                    ip_whitelist: Vec::new(),
                    group: "default".to_string(),
                    allowed_groups: vec!["gamma".to_string(), "Beta".to_string()],
                    max_multiplier: None,
                    transforms: Vec::new(),
                },
                false,
            )
            .await
            .expect("api key created");

        state
            .monoize_store
            .create_provider(crate::monoize_routing::CreateMonoizeProviderInput {
                name: "provider".to_string(),
                provider_type: crate::monoize_routing::MonoizeProviderType::Responses,
                enabled: true,
                priority: Some(0),
                max_retries: -1,
                channel_max_retries: 0,
                channel_retry_interval_ms: 0,
                circuit_breaker_enabled: true,
                per_model_circuit_break: false,
                models: std::collections::HashMap::from([(
                    "gpt-5".to_string(),
                    crate::monoize_routing::MonoizeModelEntry {
                        redirect: None,
                        multiplier: 1.0,
                    },
                )]),
                channels: vec![crate::monoize_routing::CreateMonoizeChannelInput {
                    id: None,
                    name: "ch".to_string(),
                    base_url: "https://example.com".to_string(),
                    api_key: Some("secret".to_string()),
                    weight: 1,
                    enabled: true,
                    groups: vec!["beta".to_string(), " delta ".to_string()],
                    passive_failure_count_threshold_override: None,
                    passive_window_seconds_override: None,
                    passive_cooldown_seconds_override: None,
                    passive_rate_limit_cooldown_seconds_override: None,
                }],
                transforms: Vec::new(),
                api_type_overrides: Vec::new(),
                active_probe_enabled_override: None,
                active_probe_interval_seconds_override: None,
                active_probe_success_threshold_override: None,
                active_probe_model_override: None,
                request_timeout_ms_override: None,
            })
            .await
            .expect("provider created");

        state
            .user_store
            .db
            .write()
            .await
            .execute(state.user_store.db.stmt(
                "UPDATE users SET allowed_groups = $1 WHERE id = $2",
                vec!["not-json".into(), user.id.clone().into()],
            ))
            .await
            .expect("corrupt user groups");
        state
            .user_store
            .db
            .write()
            .await
            .execute(state.user_store.db.stmt(
                "UPDATE api_keys SET allowed_groups = $1",
                vec![r#"["Gamma"," epsilon ",""]"#.into()],
            ))
            .await
            .expect("override api key groups");
        state
            .monoize_store
            .list_all_channel_groups_json()
            .await
            .expect("channel groups query works");
        state
            .user_store
            .db
            .write()
            .await
            .execute(state.user_store.db.stmt(
                "UPDATE monoize_channels SET groups = $1",
                vec![r#"["beta"," delta ","BETA"]"#.into()],
            ))
            .await
            .expect("override channel groups");

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", session.token)).expect("header value"),
        );

        let response = list_dashboard_groups(State(state), headers)
            .await
            .expect("handler succeeds");
        let Json(body) = response;
        let body: DashboardGroupsResponse = body;

        assert_eq!(
            body.groups,
            vec![
                "beta".to_string(),
                "delta".to_string(),
                "epsilon".to_string(),
                "gamma".to_string(),
            ]
        );
    }
}

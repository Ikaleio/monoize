use crate::exact_decimal::Multiplier;
use crate::transforms::TransformRuleConfig;
use crate::users::{
    RequestCaptureMode, RequestCaptureRetention, UserStore, resolve_effective_groups,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InternalRequestSource {
    Playground,
}

impl InternalRequestSource {
    pub const fn request_kind(self) -> &'static str {
        match self {
            Self::Playground => "playground",
        }
    }
}

/// Forwarding authorization resolved from an API key or an internal dashboard source.
#[derive(Clone, Debug)]
pub struct AuthResult {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub user_role: crate::users::UserRole,
    pub api_key_id: Option<String>,
    pub api_key_name: Option<String>,
    pub internal_source: Option<InternalRequestSource>,
    pub max_multiplier: Option<Multiplier>,
    pub transforms: Vec<TransformRuleConfig>,
    pub model_redirects: Vec<crate::users::CompiledModelRedirectRule>,
    pub effective_groups: Option<Vec<String>>,
    pub model_limits_enabled: bool,
    pub model_limits: Vec<String>,
    pub ip_whitelist: Vec<String>,
    pub sub_account_enabled: bool,
    pub sub_account_balance_nano: String,
    pub reasoning_envelope_enabled: bool,
    pub request_capture_mode: RequestCaptureMode,
    pub request_capture_retention: RequestCaptureRetention,
}

#[derive(Clone)]
pub struct AuthState;

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthState {
    pub fn new() -> Self {
        Self
    }

    /// Authenticate a token using database API keys.
    ///
    /// For database API keys, the user_id is used as the tenant_id for isolation.
    pub async fn authenticate_token(
        &self,
        token: &str,
        user_store: Option<&UserStore>,
    ) -> Option<AuthResult> {
        if token.starts_with("sk-") && token.len() >= 12 {
            if let Some(store) = user_store {
                match store.validate_api_key(token).await {
                    Ok(Some((api_key, user))) => {
                        // GR-I4: API-key auth always yields a concrete ordered list;
                        // `None` is reserved for internal system traffic.
                        let effective_groups = Some(resolve_effective_groups(
                            &user.group_id,
                            api_key.use_user_group,
                            &api_key.group_ids,
                        ));
                        return Some(AuthResult {
                            tenant_id: user.id.clone(),
                            user_id: Some(user.id),
                            username: Some(user.username.clone()),
                            user_role: user.role,
                            api_key_id: Some(api_key.id),
                            api_key_name: Some(api_key.name),
                            internal_source: None,
                            max_multiplier: api_key.max_multiplier,
                            transforms: api_key.transforms,
                            model_redirects: api_key.compiled_model_redirects,
                            effective_groups,
                            model_limits_enabled: api_key.model_limits_enabled,
                            model_limits: api_key.model_limits,
                            ip_whitelist: api_key.ip_whitelist,
                            sub_account_enabled: api_key.sub_account_enabled,
                            sub_account_balance_nano: api_key.sub_account_balance_nano,
                            reasoning_envelope_enabled: api_key.reasoning_envelope_enabled,
                            request_capture_mode: api_key.request_capture_mode,
                            request_capture_retention: api_key.request_capture_retention,
                        });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!(token_prefix = &token[..token.len().min(8)], error = %e, "API key validation failed due to internal error");
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::AuthState;
    use crate::db::DbPool;
    use crate::migration::Migrator;
    use crate::users::{
        CreateApiKeyInput, CreateGroupInput, RequestCaptureMode, UserRole, UserStore,
    };
    use sea_orm_migration::MigratorTrait;

    async fn make_user_store() -> UserStore {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("db connects");
        {
            let write = db.write().await;
            Migrator::up(&*write, None).await.expect("migrates");
        }

        let (log_tx, _) = tokio::sync::broadcast::channel(1);
        UserStore::new(db, log_tx).await.expect("store creates")
    }

    fn key_input(name: &str, use_user_group: bool, group_ids: Vec<String>) -> CreateApiKeyInput {
        CreateApiKeyInput {
            name: name.to_string(),
            expires_in_days: None,
            sub_account_enabled: false,
            sub_account_balance_nano_usd: None,
            model_limits_enabled: false,
            model_limits: Vec::new(),
            ip_whitelist: Vec::new(),
            use_user_group,
            group_ids,
            max_multiplier: None,
            transforms: Vec::new(),
            model_redirects: Vec::new(),
            reasoning_envelope_enabled: true,
            request_capture_mode: RequestCaptureMode::Off,
            request_capture_retention: crate::users::RequestCaptureRetention::default(),
        }
    }

    #[tokio::test]
    async fn authenticate_token_resolves_owner_group_for_inheriting_key() {
        let store = make_user_store().await;
        let default_group_id = store.default_group_id().await.expect("default exists");
        let user = store
            .create_user("alice", "password123", UserRole::User, None)
            .await
            .expect("user created");
        let (_, token) = store
            .create_api_key_extended(
                &user.id,
                key_input("inheriting key", true, Vec::new()),
                false,
            )
            .await
            .expect("api key created");

        let auth = AuthState::new()
            .authenticate_token(&token, Some(&store))
            .await
            .expect("auth succeeds");

        assert_eq!(auth.user_id.as_deref(), Some(user.id.as_str()));
        assert_eq!(auth.effective_groups, Some(vec![default_group_id]));
    }

    #[tokio::test]
    async fn authenticate_token_preserves_key_group_order() {
        let store = make_user_store().await;
        let team_a = store
            .create_group(CreateGroupInput {
                name: "team-a".to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 1,
                billing_ratio: None,
            })
            .await
            .expect("group created");
        let team_b = store
            .create_group(CreateGroupInput {
                name: "team-b".to_string(),
                description: String::new(),
                user_selectable: false,
                sort_order: 2,
                billing_ratio: None,
            })
            .await
            .expect("group created");

        let user = store
            .create_user("bob", "password123", UserRole::User, None)
            .await
            .expect("user created");
        let (_, token) = store
            .create_api_key_extended(
                &user.id,
                key_input(
                    "explicit key",
                    false,
                    vec![team_b.id.clone(), team_a.id.clone()],
                ),
                true,
            )
            .await
            .expect("api key created");

        let auth = AuthState::new()
            .authenticate_token(&token, Some(&store))
            .await
            .expect("auth succeeds");
        assert_eq!(
            auth.effective_groups,
            Some(vec![team_b.id.clone(), team_a.id.clone()])
        );
    }
}

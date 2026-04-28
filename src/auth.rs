use crate::transforms::TransformRuleConfig;
use crate::users::{UserStore, compute_effective_groups};

/// Result of authentication containing the tenant_id and optionally the user_id
/// if authenticated via database API key.
#[derive(Clone, Debug)]
pub struct AuthResult {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub user_role: crate::users::UserRole,
    pub api_key_id: Option<String>,
    pub max_multiplier: Option<f64>,
    pub transforms: Vec<TransformRuleConfig>,
    pub model_redirects: Vec<crate::users::ModelRedirectRule>,
    pub effective_groups: Option<Vec<String>>,
    pub model_limits_enabled: bool,
    pub model_limits: Vec<String>,
    pub ip_whitelist: Vec<String>,
    pub sub_account_enabled: bool,
    pub sub_account_balance_nano: String,
    pub reasoning_envelope_enabled: bool,
}

impl AuthResult {
    pub fn can_bypass_unpriced_models(&self) -> bool {
        self.user_role.can_manage_users()
    }
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
                        let effective_groups =
                            compute_effective_groups(&user.allowed_groups, &api_key.allowed_groups);
                        return Some(AuthResult {
                            tenant_id: user.id.clone(),
                            user_id: Some(user.id),
                            username: Some(user.username.clone()),
                            user_role: user.role,
                            api_key_id: Some(api_key.id),
                            max_multiplier: api_key.max_multiplier,
                            transforms: api_key.transforms,
                            model_redirects: api_key.model_redirects,
                            effective_groups,
                            model_limits_enabled: api_key.model_limits_enabled,
                            model_limits: api_key.model_limits,
                            ip_whitelist: api_key.ip_whitelist,
                            sub_account_enabled: api_key.sub_account_enabled,
                            sub_account_balance_nano: api_key.sub_account_balance_nano,
                            reasoning_envelope_enabled: api_key.reasoning_envelope_enabled,
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
    use crate::users::{CreateApiKeyInput, UserRole, UserStore};
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

    #[tokio::test]
    async fn authenticate_token_returns_none_for_unrestricted_effective_groups() {
        let store = make_user_store().await;
        let user = store
            .create_user("alice", "password123", UserRole::User, &[])
            .await
            .expect("user created");
        let (_, token) = store
            .create_api_key_extended(
                &user.id,
                CreateApiKeyInput {
                    name: "default key".to_string(),
                    expires_in_days: None,
                    sub_account_enabled: false,
                    model_limits_enabled: false,
                    model_limits: Vec::new(),
                    ip_whitelist: Vec::new(),
                    allowed_groups: Vec::new(),
                    max_multiplier: None,
                    transforms: Vec::new(),
                    model_redirects: Vec::new(),
                    reasoning_envelope_enabled: true,
                },
                false,
            )
            .await
            .expect("api key created");

        let auth = AuthState::new()
            .authenticate_token(&token, Some(&store))
            .await
            .expect("auth succeeds");

        assert_eq!(auth.user_id.as_deref(), Some(user.id.as_str()));
        assert_eq!(auth.effective_groups, None);
    }

    #[tokio::test]
    async fn authenticate_token_returns_restricted_effective_groups_without_enforcement() {
        let store = make_user_store().await;
        let user = store
            .create_user(
                "bob",
                "password123",
                UserRole::User,
                &[" Team-B ".to_string(), "team-a".to_string()],
            )
            .await
            .expect("user created");

        let (_, intersecting_token) = store
            .create_api_key_extended(
                &user.id,
                CreateApiKeyInput {
                    name: "intersection key".to_string(),
                    expires_in_days: None,
                    sub_account_enabled: false,
                    model_limits_enabled: false,
                    model_limits: Vec::new(),
                    ip_whitelist: Vec::new(),
                    allowed_groups: vec![" TEAM-B ".to_string()],
                    max_multiplier: None,
                    transforms: Vec::new(),
                    model_redirects: Vec::new(),
                    reasoning_envelope_enabled: true,
                },
                false,
            )
            .await
            .expect("api key created");

        let intersecting_auth = AuthState::new()
            .authenticate_token(&intersecting_token, Some(&store))
            .await
            .expect("auth succeeds");
        assert_eq!(
            intersecting_auth.effective_groups,
            Some(vec!["team-b".to_string()])
        );

        store
            .update_user(
                &user.id,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(&["team-a".to_string()]),
            )
            .await
            .expect("user groups updated");

        let (_, disjoint_token) = store
            .create_api_key_extended(
                &user.id,
                CreateApiKeyInput {
                    name: "disjoint key".to_string(),
                    expires_in_days: None,
                    sub_account_enabled: false,
                    model_limits_enabled: false,
                    model_limits: Vec::new(),
                    ip_whitelist: Vec::new(),
                    allowed_groups: vec!["team-a".to_string()],
                    max_multiplier: None,
                    transforms: Vec::new(),
                    model_redirects: Vec::new(),
                    reasoning_envelope_enabled: true,
                },
                false,
            )
            .await
            .expect("api key created");

        store
            .update_user(
                &user.id,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(&["team-b".to_string()]),
            )
            .await
            .expect("user groups updated");

        let disjoint_auth = AuthState::new()
            .authenticate_token(&disjoint_token, Some(&store))
            .await
            .expect("auth succeeds");
        assert_eq!(disjoint_auth.effective_groups, Some(Vec::new()));
    }
}

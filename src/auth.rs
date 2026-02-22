use crate::transforms::TransformRuleConfig;
use crate::users::UserStore;

/// Result of authentication containing the tenant_id and optionally the user_id
/// if authenticated via database API key.
#[derive(Clone, Debug)]
pub struct AuthResult {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub api_key_id: Option<String>,
    pub max_multiplier: Option<f64>,
    pub transforms: Vec<TransformRuleConfig>,
    pub model_limits_enabled: bool,
    pub model_limits: Vec<String>,
}

#[derive(Clone)]
pub struct AuthState;

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
                        return Some(AuthResult {
                            tenant_id: user.id.clone(),
                            user_id: Some(user.id),
                            username: Some(user.username.clone()),
                            api_key_id: Some(api_key.id),
                            max_multiplier: api_key.max_multiplier,
                            transforms: api_key.transforms,
                            model_limits_enabled: api_key.model_limits_enabled,
                            model_limits: api_key.model_limits,
                        });
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            }
        }
        None
    }
}

use crate::app::AppState;
use crate::captcha::{BuiltInCapError, BuiltInRedeemRequest};
use crate::error::{AppError, AppResult};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn create_captcha_challenge(State(state): State<AppState>) -> AppResult<Response> {
    require_builtin_captcha(&state).await?;
    let challenge = state
        .cap_verifier
        .create_builtin_challenge()
        .map_err(map_builtin_error)?;
    Ok(Json(challenge).into_response())
}

pub async fn redeem_captcha_challenge(
    State(state): State<AppState>,
    Json(request): Json<BuiltInRedeemRequest>,
) -> AppResult<Response> {
    require_builtin_captcha(&state).await?;
    let response = state
        .cap_verifier
        .redeem_builtin_challenge(&request)
        .map_err(map_builtin_error)?;
    Ok(Json(response).into_response())
}

async fn require_builtin_captcha(state: &AppState) -> AppResult<()> {
    let enabled = state
        .settings_store
        .is_captcha_enabled()
        .await
        .map_err(|error| {
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", error)
        })?;
    if !enabled {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "captcha_disabled",
            "CAPTCHA is disabled",
        ));
    }
    if !state.cap_verifier.is_builtin() {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "built-in CAPTCHA is not active",
        ));
    }
    Ok(())
}

fn map_builtin_error(error: BuiltInCapError) -> AppError {
    match error {
        BuiltInCapError::NotBuiltIn => AppError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "built-in CAPTCHA is not active",
        ),
        BuiltInCapError::Capacity => AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "captcha_unavailable",
            "CAPTCHA capacity is temporarily exhausted",
        ),
    }
}

use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::net::SocketAddr;
use time::Duration;
use tracing::error;
use validator::Validate;

use crate::{
    auth::{
        action_codes::{create_action_code, verify_action_code},
        backend::{AuthSession, Credentials},
        password::hash_password,
        tokens::{consume_token, create_token},
    },
    error::AppError,
    logging::audit::{extract_client_info, record_event},
    models::user::{User, UserProfile},
    state::AppState,
};

#[derive(Deserialize, Validate)]
pub struct RegisterPayload {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,
    #[validate(length(min = 8, message = "Password must be at least 8 characters long"))]
    pub password: String,
    pub display_name: Option<String>,
}

async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    _auth_session: AuthSession,
    Json(payload): Json<RegisterPayload>,
) -> Result<impl IntoResponse, AppError> {
    payload.validate().map_err(|e| AppError::Validation(e.to_string()))?;

    let hash = hash_password(payload.password).await.map_err(AppError::Other)?;

    let user: User = match sqlx::query_as(
        r#"
        INSERT INTO users (email, password_hash, display_name)
        VALUES ($1, $2, $3)
        RETURNING *
        "#,
    )
    .bind(&payload.email)
    .bind(&hash)
    .bind(&payload.display_name)
    .fetch_one(&state.db)
    .await {
        Ok(user) => user,
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            return Err(AppError::Conflict);
        }
        Err(e) => return Err(AppError::Other(anyhow::anyhow!(e))),
    };

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    record_event(&state.db, Some(user.id), "register", ip, ua).await;

    // Phase 2: Send verification email
    let raw_token = create_token(&state.db, user.id, "verify", Duration::days(1))
        .await
        .map_err(AppError::Other)?;

    let verify_link = format!("{}/auth/verify?token={}", state.config.app_base_url, raw_token);

    if let Err(e) = state.mailer.send_verification(&user.email, &verify_link).await {
        error!("Failed to send verification email to {}: {:?}", user.email, e);
    }

    let profile = UserProfile::from(user);
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn login(
    mut auth_session: AuthSession,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(creds): Json<Credentials>,
) -> Result<impl IntoResponse, AppError> {
    let user = match auth_session.authenticate(creds).await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))? {
        Some(user) => user,
        None => {
            let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
            record_event(&state.db, None, "login_fail", ip, ua).await;
            return Err(AppError::Unauthorized);
        }
    };

    if !user.email_verified {
        let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
        record_event(&state.db, None, "login_fail", ip, ua).await;
        return Err(AppError::Forbidden("email not verified".to_string()));
    }

    if let Err(e) = auth_session.login(&user).await {
        let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
        record_event(&state.db, None, "login_fail", ip, ua).await;
        return Err(AppError::Other(anyhow::anyhow!(e)));
    }

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    record_event(&state.db, Some(user.id), "login_ok", ip, ua).await;

    let profile = UserProfile::from(user);
    Ok((StatusCode::OK, Json(profile)))
}

async fn logout(
    mut auth_session: AuthSession,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let uid = auth_session.user.as_ref().map(|u| u.id);
    auth_session.logout().await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    record_event(&state.db, uid, "logout", ip, ua).await;

    Ok(StatusCode::NO_CONTENT)
}

async fn me(auth_session: AuthSession) -> Result<impl IntoResponse, AppError> {
    match auth_session.user {
        Some(user) => {
            let profile = UserProfile::from(user);
            Ok((StatusCode::OK, Json(profile)))
        }
        None => Err(AppError::Unauthorized),
    }
}

#[derive(Deserialize)]
pub struct TokenQuery {
    pub token: String,
}

async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<TokenQuery>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = consume_token(&state.db, &query.token, "verify")
        .await
        .map_err(AppError::Other)?;

    match user_id {
        Some(uid) => {
            sqlx::query("UPDATE users SET email_verified = true WHERE id = $1")
                .bind(uid)
                .execute(&state.db)
                .await?;

            let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
            record_event(&state.db, Some(uid), "verify", ip, ua).await;

            let redirect_url = format!("{}/login?verified=1", state.config.frontend_url);
            Ok(Redirect::to(&redirect_url).into_response())
        }
        None => Err(AppError::BadRequest("invalid or expired token".to_string())),
    }
}

#[derive(Deserialize)]
pub struct ForgotPayload {
    pub email: String,
}

async fn forgot_password(
    State(state): State<AppState>,
    Json(payload): Json<ForgotPayload>,
) -> Result<impl IntoResponse, AppError> {
    let user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE email = $1 AND deleted_at IS NULL")
        .bind(&payload.email)
        .fetch_optional(&state.db)
        .await?;

    if let Some(u) = user {
        let raw_token = match create_token(&state.db, u.id, "reset", Duration::hours(1)).await {
            Ok(t) => t,
            Err(e) => {
                error!("Failed to create reset token for {}: {:?}", u.email, e);
                return Ok(StatusCode::OK);
            }
        };

        let reset_link = format!("{}/reset-password?token={}", state.config.frontend_url, raw_token);

        if let Err(e) = state.mailer.send_reset(&u.email, &reset_link).await {
            error!("Failed to send reset email to {}: {:?}", u.email, e);
        }
    }

    Ok(StatusCode::OK)
}

#[derive(Deserialize, Validate)]
pub struct ResetPayload {
    pub token: String,
    #[validate(length(min = 8, message = "Password must be at least 8 characters long"))]
    pub new_password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<ResetPayload>,
) -> Result<impl IntoResponse, AppError> {
    payload.validate().map_err(|e| AppError::Validation(e.to_string()))?;

    let user_id = consume_token(&state.db, &payload.token, "reset")
        .await
        .map_err(AppError::Other)?;

    match user_id {
        Some(uid) => {
            let hash = hash_password(payload.new_password).await.map_err(AppError::Other)?;

            sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
                .bind(hash)
                .bind(uid)
                .execute(&state.db)
                .await?;

            let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
            record_event(&state.db, Some(uid), "reset", ip, ua).await;

            Ok(StatusCode::OK)
        }
        None => Err(AppError::BadRequest("invalid or expired token".to_string())),
    }
}

async fn request_account_delete(
    auth_session: AuthSession,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;
    
    let code = create_action_code(
        &state.db,
        user.id,
        "delete_account",
        Duration::minutes(10),
    ).await?;
    
    if let Err(e) = state.mailer.send_action_code(&user.email, &code, "Account Deletion").await {
        error!("Failed to send action code to {}: {:?}", user.email, e);
    }
    
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct DeleteConfirmPayload {
    pub code: String,
}

async fn confirm_account_delete(
    mut auth_session: AuthSession,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(payload): Json<DeleteConfirmPayload>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.clone().ok_or(AppError::Unauthorized)?;
    
    verify_action_code(
        &state.db,
        user.id,
        "delete_account",
        &payload.code,
    ).await?;
    
    // Soft delete the user
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(user.id)
        .execute(&state.db)
        .await?;
        
    // Destroy the current session.
    // Note: because of Phase 6 guard, once deleted_at is set the user is immediately locked out everywhere else.
    auth_session.logout().await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    
    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    record_event(&state.db, Some(user.id), "account_deleted", ip, ua).await;
    
    Ok(StatusCode::OK)
}

async fn request_password_change(
    auth_session: AuthSession,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;
    
    if user.password_hash.is_none() {
        return Err(AppError::BadRequest("no password set for this account; you sign in with Google.".to_string()));
    }
    
    let code = create_action_code(
        &state.db,
        user.id,
        "change_password",
        Duration::minutes(10),
    ).await?;
    
    if let Err(e) = state.mailer.send_action_code(&user.email, &code, "Password Change").await {
        error!("Failed to send action code to {}: {:?}", user.email, e);
    }
    
    Ok(StatusCode::OK)
}

#[derive(Deserialize, Validate)]
pub struct PasswordChangeConfirmPayload {
    pub current_password: String,
    #[validate(length(min = 8, message = "Password must be at least 8 characters long"))]
    pub new_password: String,
    pub code: String,
}

async fn confirm_password_change(
    mut auth_session: AuthSession,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(payload): Json<PasswordChangeConfirmPayload>,
) -> Result<impl IntoResponse, AppError> {
    payload.validate().map_err(|e| AppError::Validation(e.to_string()))?;

    let user = auth_session.user.clone().ok_or(AppError::Unauthorized)?;

    let hash = user.password_hash.as_deref().ok_or_else(|| AppError::Unauthorized)?;
    let is_valid_password = crate::auth::password::verify_password(payload.current_password, hash.to_string()).await;

    let is_valid_code = verify_action_code(&state.db, user.id, "change_password", &payload.code).await.is_ok();

    if !is_valid_password || !is_valid_code {
        return Err(AppError::Unauthorized); // Generic error to hide which factor failed
    }

    let new_hash = hash_password(payload.new_password)
        .await
        .map_err(AppError::Other)?;

    let updated_user: User = sqlx::query_as(
        "UPDATE users SET password_hash = $1 WHERE id = $2 RETURNING *"
    )
    .bind(&new_hash)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    // Log in with the updated user to update the auth hash in the current session.
    // Because the auth hash (password hash) changed, all OTHER existing sessions for this user
    // will be automatically invalidated by axum-login when they try to authenticate.
    // By re-logging in here, we keep THIS current session valid so the user isn't logged out.
    auth_session.login(&updated_user).await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    record_event(&state.db, Some(user.id), "password_changed", ip, ua).await;

    Ok(StatusCode::OK)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/verify", get(verify))
        .route("/password/forgot", post(forgot_password))
        .route("/password/reset", post(reset_password))
        .route("/password/change/request", post(request_password_change))
        .route("/password/change/confirm", post(confirm_password_change))
        .route("/account/delete/request", post(request_account_delete))
        .route("/account/delete/confirm", post(confirm_account_delete))
}

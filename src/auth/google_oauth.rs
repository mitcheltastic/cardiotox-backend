use axum::{
    extract::{ConnectInfo, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::Deserialize;
use std::net::SocketAddr;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    auth::backend::AuthSession,
    error::AppError,
    logging::audit::{extract_client_info, record_event},
    models::user::User,
    state::AppState,
};

fn build_http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))
}

fn redirect_to_frontend(url: &str) -> Response {
    let mut response = Redirect::to(url).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    response.headers_mut().insert(
        axum::http::header::PRAGMA,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response
}

async fn google_auth(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<impl IntoResponse, AppError> {
    let client = BasicClient::new(ClientId::new(state.config.google_client_id.clone()))
        .set_client_secret(ClientSecret::new(state.config.google_client_secret.clone()))
        .set_auth_uri(
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".into())
                .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        )
        .set_token_uri(
            TokenUrl::new("https://oauth2.googleapis.com/token".into())
                .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(state.config.google_redirect_url.clone())
                .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        );

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();

    let (auth_url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();

    auth_session
        .session
        .insert("pkce_verifier", verifier.secret())
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    auth_session
        .session
        .insert("csrf_state", csrf.secret())
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    if let Some(session_id) = auth_session.session.id() {
        info!("GET /auth/google (start): Session ID = {}", session_id);
    } else {
        info!("GET /auth/google (start): Session ID = None");
    }

    let mut response = Redirect::to(auth_url.as_str()).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    response.headers_mut().insert(
        axum::http::header::PRAGMA,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}

#[derive(Deserialize)]
pub struct AuthCallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Deserialize)]
pub struct GoogleUserInfo {
    pub sub: String,
    pub email: String,
    pub email_verified: bool,
    pub name: Option<String>,
}

async fn google_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut auth_session: AuthSession,
    Query(query): Query<AuthCallbackQuery>,
) -> Result<impl IntoResponse, AppError> {
    info!("GET /auth/google/callback HIT. state len: {}, code len: {}", query.state.len(), query.code.len());

    let frontend_base = state.config.frontend_url.trim_end_matches('/');
    let error_redirect_url = format!("{}/login?error=invalid", frontend_base);
    let success_redirect_url = format!("{}/login?login=success", frontend_base);

    if let Some(session_id) = auth_session.session.id() {
        info!("Callback Session ID = {}", session_id);
    } else {
        info!("Callback Session ID = None");
    }

    let stored_csrf: Result<Option<String>, _> = auth_session
        .session
        .get("csrf_state")
        .await;

    let stored_pkce: Result<Option<String>, _> = auth_session
        .session
        .get("pkce_verifier")
        .await;

    let stored_csrf = match stored_csrf {
        Ok(v) => {
            info!("session csrf_state: {}", if v.is_some() { "present" } else { "ABSENT" });
            v
        },
        Err(e) => {
            error!("Failed to get csrf_state from session: {}", e);
            warn!("Session state read error branch triggered");
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    let stored_pkce = match stored_pkce {
        Ok(v) => {
            info!("session pkce_verifier: {}", if v.is_some() { "present" } else { "ABSENT" });
            v
        },
        Err(e) => {
            error!("Failed to get pkce_verifier from session: {}", e);
            warn!("Session pkce read error branch triggered");
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    let state_matches = stored_csrf.as_deref() == Some(&query.state);
    info!("State comparison: {}", if state_matches { "matched" } else { "MISMATCH" });

    if !state_matches {
        warn!("State mismatch error branch triggered");
        auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
        auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
        return Ok(redirect_to_frontend(&error_redirect_url));
    }

    let pkce_secret = match stored_pkce {
        Some(s) => s,
        None => {
            warn!("PKCE verifier missing error branch triggered");
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    let client = BasicClient::new(ClientId::new(state.config.google_client_id.clone()))
        .set_client_secret(ClientSecret::new(state.config.google_client_secret.clone()))
        .set_auth_uri(
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".into())
                .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        )
        .set_token_uri(
            TokenUrl::new("https://oauth2.googleapis.com/token".into())
                .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(state.config.google_redirect_url.clone())
                .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?,
        );

    let http = match build_http_client() {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to build HTTP client: {:?}", e);
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    let token_res = match client
        .exchange_code(AuthorizationCode::new(query.code))
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_secret))
        .request_async(&http)
        .await
    {
        Ok(tokens) => tokens,
        Err(e) => {
            error!("Token exchange failed with error: {:?}", e);
            warn!("Token exchange error branch triggered");
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    let user_info_res = match http
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .header(
            "Authorization",
            format!("Bearer {}", token_res.access_token().secret()),
        )
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            error!("Userinfo request failed with error: {:?}", e);
            warn!("Userinfo fetch error branch triggered");
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    let status = user_info_res.status();
    info!("Userinfo fetch HTTP status: {}", status);
    
    if !status.is_success() {
        let body = user_info_res.text().await.unwrap_or_else(|_| "Failed to read body".to_string());
        error!("Userinfo request failed. Response body: {}", body);
        warn!("Userinfo fetch non-success error branch triggered");
        auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
        auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
        return Ok(redirect_to_frontend(&error_redirect_url));
    }

    let user_info: GoogleUserInfo = match user_info_res.json().await {
        Ok(info) => info,
        Err(e) => {
            error!("Userinfo parse failed with error: {:?}", e);
            warn!("Userinfo parse error branch triggered");
            auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
            auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
            return Ok(redirect_to_frontend(&error_redirect_url));
        }
    };

    info!("Email verified: {}", user_info.email_verified);

    if !user_info.email_verified {
        warn!("Email not verified error branch triggered");
        auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
        auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();
        return Ok(redirect_to_frontend(&error_redirect_url));
    }

    let mut tx = state.db.begin().await.map_err(|e| {
        warn!("DB begin error branch triggered");
        AppError::Database(e)
    })?;

    let oauth_account: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM oauth_accounts WHERE provider = 'google' AND provider_user_id = $1",
    )
    .bind(&user_info.sub)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        warn!("DB select oauth_account error branch triggered");
        AppError::Database(e)
    })?;

    let user: User = if let Some(user_id) = oauth_account {
        sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(AppError::Database)?
    } else {
        let existing_user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE email = $1")
            .bind(&user_info.email)
            .fetch_optional(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        let user_id = if let Some(u) = existing_user {
            u.id
        } else {
            let u: User = sqlx::query_as(
                r#"
                INSERT INTO users (email, email_verified, display_name)
                VALUES ($1, true, $2)
                RETURNING *
                "#,
            )
            .bind(&user_info.email)
            .bind(&user_info.name)
            .fetch_one(&mut *tx)
            .await
            .map_err(AppError::Database)?;
            u.id
        };

        sqlx::query(
            r#"
            INSERT INTO oauth_accounts (user_id, provider, provider_user_id)
            VALUES ($1, 'google', $2)
            "#,
        )
        .bind(user_id)
        .bind(&user_info.sub)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

        sqlx::query_as("SELECT * FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(AppError::Database)?
    };

    // Check if account is deactivated / soft-deleted
    if user.deleted_at.is_some() {
        warn!("Deactivated user {} ({}) attempted Google OAuth login", user.id, user.email);
        tx.rollback().await.ok();

        auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
        auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();

        let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
        record_event(&state.db, Some(user.id), "oauth_login_deactivated", ip, ua).await;

        return Ok(redirect_to_frontend(&error_redirect_url));
    }

    tx.commit().await.map_err(|e| {
        warn!("DB commit error branch triggered");
        AppError::Database(e)
    })?;

    auth_session
        .login(&user)
        .await
        .map_err(|e| {
            warn!("Session login error branch triggered");
            AppError::Other(anyhow::anyhow!("Session login failed: {:?}", e))
        })?;

    auth_session.session.remove::<serde_json::Value>("csrf_state").await.ok();
    auth_session.session.remove::<serde_json::Value>("pkce_verifier").await.ok();

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    record_event(&state.db, Some(user.id), "oauth_login", ip, ua).await;

    Ok(redirect_to_frontend(&success_redirect_url))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/google", get(google_auth))
        .route("/google/callback", get(google_callback))
}

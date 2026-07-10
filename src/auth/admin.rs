use axum::{
    extract::FromRequestParts,
    http::request::Parts,
};

use crate::{
    auth::backend::AuthSession,
    error::AppError,
    models::user::User,
};

pub struct AdminUser(pub User);

impl<S> FromRequestParts<S> for AdminUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_session = AuthSession::from_request_parts(parts, state)
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("Failed to extract AuthSession: {:?}", e)))?;

        match auth_session.user {
            Some(user) => {
                if user.role == "admin" {
                    Ok(AdminUser(user))
                } else {
                    Err(AppError::Forbidden("admin access required".to_string()))
                }
            }
            None => Err(AppError::Unauthorized),
        }
    }
}

use axum_login::AuthnBackend;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    auth::password::{verify_password, DUMMY_HASH},
    error::AppError,
    models::user::User,
};

#[derive(Clone)]
pub struct Backend {
    pub db: PgPool,
}

#[derive(Deserialize, Clone)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = AppError;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        // Filter out soft-deleted users here so they cannot log in.
        // This is what makes soft-delete secure.
        let user: Option<User> = sqlx::query_as("SELECT * FROM users WHERE email = $1 AND deleted_at IS NULL")
            .bind(&creds.email)
            .fetch_optional(&self.db)
            .await?;

        match user {
            Some(u) => {
                let hash = u.password_hash.clone();
                match hash {
                    Some(h) => {
                        let is_valid = verify_password(creds.password, h).await;
                        if is_valid {
                            Ok(Some(u))
                        } else {
                            Ok(None)
                        }
                    }
                    None => {
                        // User exists but has no password
                        let _ = verify_password(creds.password, DUMMY_HASH.clone()).await;
                        Ok(None)
                    }
                }
            }
            None => {
                // User not found
                let _ = verify_password(creds.password, DUMMY_HASH.clone()).await;
                Ok(None)
            }
        }
    }

    async fn get_user(&self, user_id: &Uuid) -> Result<Option<Self::User>, Self::Error> {
        // Existing sessions of soft-deleted users stop resolving here -> they're logged out
        let user = sqlx::query_as("SELECT * FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?;
        Ok(user)
    }
}

pub type AuthSession = axum_login::AuthSession<Backend>;

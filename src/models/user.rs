use axum_login::AuthUser;
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub password_hash: Option<String>,
    pub display_name: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub role: String,
    pub deleted_at: Option<OffsetDateTime>,
}

impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("password_hash", &"***")
            .field("display_name", &self.display_name)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("role", &self.role)
            .field("deleted_at", &self.deleted_at)
            .finish()
    }
}

impl AuthUser for User {
    type Id = Uuid;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.password_hash.as_deref().unwrap_or("").as_bytes()
    }
}

#[derive(Serialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub display_name: Option<String>,
    pub role: String,
}

impl From<User> for UserProfile {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            email_verified: user.email_verified,
            display_name: user.display_name,
            role: user.role,
        }
    }
}

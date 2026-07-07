use anyhow::Result;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn hash(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

pub async fn create_token(
    db: &PgPool,
    user_id: Uuid,
    kind: &str,
    ttl: Duration,
) -> Result<String> {
    let raw = generate();
    let hashed = hash(&raw);
    let expires_at = OffsetDateTime::now_utc() + ttl;

    sqlx::query(
        r#"
        INSERT INTO email_tokens (user_id, token_hash, kind, expires_at)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(hashed)
    .bind(kind)
    .bind(expires_at)
    .execute(db)
    .await?;

    Ok(raw)
}

pub async fn consume_token(
    db: &PgPool,
    raw: &str,
    kind: &str,
) -> Result<Option<Uuid>> {
    let hashed = hash(raw);
    
    // consume in a transaction
    let mut tx = db.begin().await?;

    let user_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT user_id FROM email_tokens
        WHERE token_hash = $1 AND kind = $2 AND used_at IS NULL AND expires_at > now()
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(&hashed)
    .bind(kind)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(uid) = user_id {
        sqlx::query(
            r#"
            UPDATE email_tokens SET used_at = now() WHERE token_hash = $1 AND kind = $2
            "#,
        )
        .bind(&hashed)
        .bind(kind)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(Some(uid))
    } else {
        tx.rollback().await?;
        Ok(None)
    }
}

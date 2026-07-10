use rand::Rng;
use rand::RngExt;
use sha2::{Sha256, Digest};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::AppError;

pub fn generate_code() -> String {
    let mut rng = rand::rng(); // rand 0.9+ uses rand::rng()
    let code: u32 = rng.random_range(100_000..=999_999);
    code.to_string()
}

fn hash_code(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn create_action_code(
    db: &PgPool,
    user_id: Uuid,
    action: &str,
    ttl: Duration,
) -> Result<String, AppError> {
    let raw_code = generate_code();
    let code_hash = hash_code(&raw_code);
    let expires_at = OffsetDateTime::now_utc() + ttl;

    let mut tx = db.begin().await?;

    sqlx::query(
        "UPDATE action_codes SET used_at = now() WHERE user_id = $1 AND action = $2 AND used_at IS NULL"
    )
    .bind(user_id)
    .bind(action)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO action_codes (user_id, code_hash, action, expires_at)
        VALUES ($1, $2, $3, $4)
        "#
    )
    .bind(user_id)
    .bind(&code_hash)
    .bind(action)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(raw_code)
}

#[derive(sqlx::FromRow)]
struct ActionCodeRow {
    id: Uuid,
    code_hash: String,
    attempts: i32,
    expires_at: OffsetDateTime,
}

pub async fn verify_action_code(
    db: &PgPool,
    user_id: Uuid,
    action: &str,
    submitted_code: &str,
) -> Result<(), AppError> {
    let mut tx = db.begin().await?;

    let row: Option<ActionCodeRow> = sqlx::query_as(
        r#"
        SELECT id, code_hash, attempts, expires_at 
        FROM action_codes 
        WHERE user_id = $1 AND action = $2 AND used_at IS NULL 
        ORDER BY created_at DESC 
        LIMIT 1 
        FOR UPDATE
        "#
    )
    .bind(user_id)
    .bind(action)
    .fetch_optional(&mut *tx)
    .await?;

    let row = match row {
        Some(r) => r,
        None => return Err(AppError::BadRequest("Invalid or expired code".to_string())),
    };

    if row.expires_at < OffsetDateTime::now_utc() {
        return Err(AppError::BadRequest("Invalid or expired code".to_string()));
    }

    if row.attempts >= 5 {
        sqlx::query("UPDATE action_codes SET used_at = now() WHERE id = $1")
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(AppError::BadRequest("Too many attempts, code invalidated".to_string()));
    }

    sqlx::query("UPDATE action_codes SET attempts = attempts + 1 WHERE id = $1")
        .bind(row.id)
        .execute(&mut *tx)
        .await?;

    let expected_hash = hash_code(submitted_code);
    
    let mut is_valid = true;
    if expected_hash.len() != row.code_hash.len() {
        is_valid = false;
    } else {
        let mut result = 0;
        for (a, b) in expected_hash.bytes().zip(row.code_hash.bytes()) {
            result |= a ^ b;
        }
        if result != 0 {
            is_valid = false;
        }
    }

    if !is_valid {
        tx.commit().await?;
        return Err(AppError::BadRequest("Invalid or expired code".to_string()));
    }

    sqlx::query("UPDATE action_codes SET used_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
        
    tx.commit().await?;

    Ok(())
}

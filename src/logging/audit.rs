use axum::{
    extract::ConnectInfo,
    http::HeaderMap,
};
use sqlx::PgPool;
use std::net::SocketAddr;
use tracing::error;
use uuid::Uuid;

pub async fn record_event(
    db: &PgPool,
    user_id: Option<Uuid>,
    event: &str,
    ip: Option<String>,
    user_agent: Option<String>,
) {
    let res = sqlx::query(
        r#"
        INSERT INTO auth_events (user_id, event, ip, user_agent)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(event)
    .bind(ip)
    .bind(user_agent)
    .execute(db)
    .await;

    if let Err(e) = res {
        error!("Failed to record audit event '{}': {:?}", event, e);
    }
}

pub fn extract_client_info(
    headers: &HeaderMap,
    connect_info: Option<&ConnectInfo<SocketAddr>>,
) -> (Option<String>, Option<String>) {
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| connect_info.map(|ci| ci.0.ip().to_string()));

    (ip, user_agent)
}

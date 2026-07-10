use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    auth::admin::AdminUser,
    error::AppError,
    logging::audit::{extract_client_info, record_event},
    models::shap_log::ShapLog,
    state::AppState,
};
use std::net::SocketAddr;

#[derive(Deserialize)]
pub struct PaginationParams {
    pub limit: Option<String>,
    pub offset: Option<String>,
}

impl PaginationParams {
    pub fn limit(&self) -> i64 {
        self.limit
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(50)
            .clamp(1, 200)
    }

    pub fn offset(&self) -> i64 {
        self.offset
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
            .max(0)
    }
}

#[derive(Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[derive(Deserialize)]
pub struct UserFilter {
    pub include_deleted: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct AdminUserResponse {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub display_name: Option<String>,
    pub role: String,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

#[derive(Serialize)]
pub struct UserDetailResponse {
    #[serde(flatten)]
    pub user: AdminUserResponse,
    pub auth_events_count: i64,
    pub predictions_count: i64,
    pub shap_logs_count: i64,
}

async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(filter): Query<UserFilter>,
) -> Result<impl IntoResponse, AppError> {
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();
    let include_deleted = filter.include_deleted.as_deref() == Some("true");

    let (items, total): (Vec<AdminUserResponse>, i64) = if include_deleted {
        let items = sqlx::query_as(
            "SELECT id, email, email_verified, display_name, role, created_at, deleted_at FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM users")
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    } else {
        let items = sqlx::query_as(
            "SELECT id, email, email_verified, display_name, role, created_at, deleted_at FROM users WHERE deleted_at IS NULL ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE deleted_at IS NULL")
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    };

    Ok(Json(PaginatedResponse {
        items,
        limit,
        offset,
        total,
    }))
}

async fn get_user(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user: Option<AdminUserResponse> = sqlx::query_as(
        "SELECT id, email, email_verified, display_name, role, created_at, deleted_at FROM users WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let user = user.ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let auth_events_count: (i64,) = sqlx::query_as("SELECT count(*) FROM auth_events WHERE user_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await.unwrap_or((0,));

    let predictions_count: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs WHERE user_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await.unwrap_or((0,));

    let shap_logs_count: (i64,) = sqlx::query_as("SELECT count(*) FROM shap_logs WHERE user_id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await.unwrap_or((0,));

    Ok(Json(UserDetailResponse {
        user,
        auth_events_count: auth_events_count.0,
        predictions_count: predictions_count.0,
        shap_logs_count: shap_logs_count.0,
    }))
}

// Auth events
#[derive(Deserialize)]
pub struct AuthEventFilter {
    pub user_id: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct AuthEventRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub event: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

async fn list_auth_events(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(filter): Query<AuthEventFilter>,
) -> Result<impl IntoResponse, AppError> {
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();

    let (items, total): (Vec<AuthEventRow>, i64) = if let Some(uid) = filter.user_id.as_deref().and_then(|s| Uuid::parse_str(s).ok()) {
        let items = sqlx::query_as(
            "SELECT id, user_id, event, ip, user_agent, created_at FROM auth_events WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(uid)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM auth_events WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    } else {
        let items = sqlx::query_as(
            "SELECT id, user_id, event, ip, user_agent, created_at FROM auth_events ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM auth_events")
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    };

    Ok(Json(PaginatedResponse { items, limit, offset, total }))
}

// Prediction logs
#[derive(Deserialize)]
pub struct LogFilter {
    pub user_id: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct PredictionLogRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub input: serde_json::Value,
    pub predicted_tier: Option<String>,
    pub probabilities: Option<serde_json::Value>,
    pub latency_ms: Option<i32>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

async fn list_prediction_logs(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(filter): Query<LogFilter>,
) -> Result<impl IntoResponse, AppError> {
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();

    let (items, total): (Vec<PredictionLogRow>, i64) = if let Some(uid) = filter.user_id.as_deref().and_then(|s| Uuid::parse_str(s).ok()) {
        let items = sqlx::query_as(
            "SELECT id, user_id, input, predicted_tier, probabilities, latency_ms, created_at FROM prediction_logs WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(uid)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    } else {
        let items = sqlx::query_as(
            "SELECT id, user_id, input, predicted_tier, probabilities, latency_ms, created_at FROM prediction_logs ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs")
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    };

    Ok(Json(PaginatedResponse { items, limit, offset, total }))
}

// SHAP logs
async fn list_shap_logs(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(filter): Query<LogFilter>,
) -> Result<impl IntoResponse, AppError> {
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();

    let (items, total): (Vec<ShapLog>, i64) = if let Some(uid) = filter.user_id.as_deref().and_then(|s| Uuid::parse_str(s).ok()) {
        let items = sqlx::query_as(
            "SELECT id, user_id, prediction_id, input, predicted_class, base_value, contributions, latency_ms, created_at FROM shap_logs WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(uid)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM shap_logs WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    } else {
        let items = sqlx::query_as(
            "SELECT id, user_id, prediction_id, input, predicted_class, base_value, contributions, latency_ms, created_at FROM shap_logs ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM shap_logs")
            .fetch_one(&state.db)
            .await?;
            
        (items, total.0)
    };

    Ok(Json(PaginatedResponse { items, limit, offset, total }))
}

async fn delete_user(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    if admin.0.id == id {
        return Err(AppError::BadRequest("Use self-delete for your own account".to_string()));
    }

    let res = sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .execute(&state.db)
        .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("User not found or already deleted".to_string()));
    }

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    let user_agent = format!("Admin {} | {}", admin.0.id, ua.unwrap_or_default());
    
    record_event(&state.db, Some(id), "admin_deleted_user", ip, Some(user_agent)).await;

    Ok(StatusCode::OK)
}

async fn restore_user(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let res = sqlx::query("UPDATE users SET deleted_at = NULL WHERE id = $1 AND deleted_at IS NOT NULL")
        .bind(id)
        .execute(&state.db)
        .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("User not found or not deleted".to_string()));
    }

    let (ip, ua) = extract_client_info(&headers, Some(&ConnectInfo(addr)));
    let user_agent = format!("Admin {} | {}", admin.0.id, ua.unwrap_or_default());
    
    record_event(&state.db, Some(id), "admin_restored_user", ip, Some(user_agent)).await;

    Ok(StatusCode::OK)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user).delete(delete_user))
        .route("/users/{id}/restore", post(restore_user))
        .route("/auth-events", get(list_auth_events))
        .route("/prediction-logs", get(list_prediction_logs))
        .route("/shap-logs", get(list_shap_logs))
}

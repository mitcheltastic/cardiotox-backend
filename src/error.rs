use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;
use tracing::error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error")]
    Database(#[from] sqlx::Error),
    
    #[error("Service unavailable")]
    Unavailable,
    
    #[error("Conflict")]
    Conflict,
    
    #[error("Unauthorized")]
    Unauthorized,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad gateway: {0}")]
    BadGateway(String),

    #[error("Validation error: {0}")]
    Validation(String),
    
    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::Database(err) => {
                if let Some(db_err) = err.as_database_error() {
                    if db_err.code().as_deref() == Some("23505") { // unique violation
                        return (StatusCode::CONFLICT, Json(json!({"error": "Conflict"}))).into_response();
                    }
                }
                error!("Database error: {:?}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            AppError::Unavailable => {
                error!("Service unavailable error");
                (StatusCode::SERVICE_UNAVAILABLE, "Service unavailable".to_string())
            }
            AppError::Conflict => {
                (StatusCode::CONFLICT, "Conflict".to_string())
            }
            AppError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "Invalid credentials or unauthorized".to_string())
            }
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::BadGateway(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            AppError::Validation(msg) => {
                return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": msg}))).into_response();
            }
            AppError::Forbidden(msg) => {
                return (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response();
            }
            AppError::BadRequest(msg) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
            }
            AppError::Other(err) => {
                error!("Internal error: {:?}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
        };

        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}

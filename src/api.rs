use axum::{
    extract::State,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use tokio::time::Instant;
use tracing::error;

use crate::{
    auth::backend::AuthSession,
    error::AppError,
    services::prediction::call_gradio,
    state::AppState,
};

#[derive(Deserialize)]
pub struct PredictPayload {
    pub data: Vec<f64>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/predict", post(predict))
        .route("/explain", post(explain))
}

fn normalize_tier(raw: &str) -> String {
    let s = if let Some(idx) = raw.rfind(':') {
        raw[idx + 1..].trim()
    } else {
        raw.trim()
    };
    
    match s.to_lowercase().as_str() {
        "high" => "High".to_string(),
        "intermediate" => "Intermediate".to_string(),
        "low" => "Low".to_string(),
        _ => {
            tracing::warn!("Could not normalize predicted tier: '{}'", raw);
            s.to_string()
        }
    }
}

async fn predict(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(payload): Json<PredictPayload>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    if payload.data.len() != 11 {
        return Err(AppError::Validation("Expected exactly 11 numeric features".into()));
    }

    let start = Instant::now();
    let data_val = serde_json::to_value(&payload.data).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    
    let result = call_gradio(&state.http_client, &state.config.hf_space_base, "predict", &data_val).await?;
    let elapsed = start.elapsed().as_millis() as i32;

    let predicted_tier = result.as_array()
        .and_then(|arr| arr.get(1))
        .and_then(|v| v.as_str())
        .map(normalize_tier);
    
    let probabilities = result.as_array()
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.get("confidences"))
        .cloned();

    if probabilities.is_none() {
        tracing::debug!("Probabilities were null. Raw Gradio data array: {:?}", result);
    }

    let input_json = serde_json::to_value(&payload.data).unwrap();

    let db = state.db.clone();
    tokio::spawn(async move {
        let res = sqlx::query(
            r#"
            INSERT INTO prediction_logs (user_id, input, predicted_tier, probabilities, latency_ms)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(user.id)
        .bind(&input_json)
        .bind(&predicted_tier)
        .bind(&probabilities)
        .bind(elapsed)
        .execute(&db)
        .await;

        if let Err(e) = res {
            error!("Failed to record prediction log: {:?}", e);
        }
    });

    Ok(Json(serde_json::json!({ "data": result })))
}

async fn explain(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(payload): Json<PredictPayload>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    if payload.data.len() != 11 {
        return Err(AppError::Validation("Expected exactly 11 numeric features".into()));
    }

    let start = Instant::now();
    let data_val = serde_json::to_value(&payload.data).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    
    let result = call_gradio(&state.http_client, &state.config.hf_space_base, "explain", &data_val).await?;
    let elapsed = start.elapsed().as_millis() as i32;

    let predicted_tier = result.as_array()
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.get("predicted_class"))
        .and_then(|v| v.as_str())
        .map(normalize_tier);

    let input_json = serde_json::to_value(&payload.data).unwrap();

    let db = state.db.clone();
    tokio::spawn(async move {
        let res = sqlx::query(
            r#"
            INSERT INTO prediction_logs (user_id, input, predicted_tier, latency_ms)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user.id)
        .bind(&input_json)
        .bind(&predicted_tier)
        .bind(elapsed)
        .execute(&db)
        .await;

        if let Err(e) = res {
            error!("Failed to record explain log: {:?}", e);
        }
    });

    Ok(Json(serde_json::json!({ "data": result })))
}

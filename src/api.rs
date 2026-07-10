use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tracing::error;

use crate::{
    admin::{PaginatedResponse, PaginationParams},
    auth::backend::AuthSession,
    error::AppError,
    services::prediction::call_gradio,
    state::AppState,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct PredictPayload {
    pub data: Vec<f64>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/predict", post(predict))
        .route("/explain", post(explain))
        .route("/history/predictions", get(my_predictions))
        .route("/history/explanations", get(my_explanations))
        .route("/history/summary", get(my_summary))
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

    let predicted_class = result.as_array()
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.get("predicted_class"))
        .and_then(|v| v.as_str())
        .map(normalize_tier);

    let base_value = result.as_array()
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.get("base_value"))
        .and_then(|v| v.as_f64());

    let contributions = result.as_array()
        .and_then(|arr| arr.get(0))
        .and_then(|v| v.get("contributions"))
        .cloned();

    let input_json = serde_json::to_value(&payload.data).unwrap();

    let db = state.db.clone();
    tokio::spawn(async move {
        let res = sqlx::query(
            r#"
            INSERT INTO shap_logs (user_id, prediction_id, input, predicted_class, base_value, contributions, latency_ms)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(user.id)
        .bind(None::<uuid::Uuid>)
        .bind(&input_json)
        .bind(&predicted_class)
        .bind(base_value)
        .bind(&contributions)
        .bind(elapsed)
        .execute(&db)
        .await;

        if let Err(e) = res {
            error!("Failed to record explain log: {:?}", e);
        }
    });

    Ok(Json(serde_json::json!({ "data": result })))
}

// ----------------------------------------------------------------------------
// History Endpoints
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct HistoryFilter {
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct HistoryPredictionRow {
    pub id: Uuid,
    pub input: serde_json::Value,
    pub predicted_tier: Option<String>,
    pub probabilities: Option<serde_json::Value>,
    pub latency_ms: Option<i32>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

async fn my_predictions(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(filter): Query<HistoryFilter>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();

    let items = sqlx::query_as::<_, HistoryPredictionRow>(
        "SELECT id, input, predicted_tier, probabilities, latency_ms, created_at FROM prediction_logs WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(user.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(PaginatedResponse { items, limit, offset, total: total.0 }))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct HistoryExplanationRow {
    pub id: Uuid,
    pub prediction_id: Option<Uuid>,
    pub input: serde_json::Value,
    pub predicted_class: Option<String>,
    pub base_value: Option<f64>,
    pub contributions: Option<serde_json::Value>,
    pub latency_ms: Option<i32>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

async fn my_explanations(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(filter): Query<HistoryFilter>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();

    let items = sqlx::query_as::<_, HistoryExplanationRow>(
        "SELECT id, prediction_id, input, predicted_class, base_value, contributions, latency_ms, created_at FROM shap_logs WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    )
    .bind(user.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM shap_logs WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(PaginatedResponse { items, limit, offset, total: total.0 }))
}

#[derive(Serialize)]
pub struct HistorySummaryResponse {
    pub total_predictions: i64,
    pub total_explanations: i64,
    pub tier_counts: serde_json::Value,
}

async fn my_summary(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    let pred_count: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;

    let expl_count: (i64,) = sqlx::query_as("SELECT count(*) FROM shap_logs WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;

    #[derive(sqlx::FromRow)]
    struct TierCount {
        predicted_tier: Option<String>,
        count: i64,
    }
    
    let tier_rows = sqlx::query_as::<_, TierCount>("SELECT predicted_tier, count(*) as count FROM prediction_logs WHERE user_id = $1 GROUP BY predicted_tier")
        .bind(user.id)
        .fetch_all(&state.db)
        .await?;
        
    let mut tier_counts = serde_json::Map::new();
    for row in tier_rows {
        let key = row.predicted_tier.unwrap_or_else(|| "Unknown".to_string());
        tier_counts.insert(key, serde_json::json!(row.count));
    }

    Ok(Json(HistorySummaryResponse {
        total_predictions: pred_count.0,
        total_explanations: expl_count.0,
        tier_counts: serde_json::Value::Object(tier_counts),
    }))
}

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ShapLog {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub prediction_id: Option<Uuid>,
    pub input: serde_json::Value,
    pub predicted_class: Option<String>,
    pub base_value: Option<f64>,
    pub contributions: Option<serde_json::Value>,
    pub latency_ms: Option<i32>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

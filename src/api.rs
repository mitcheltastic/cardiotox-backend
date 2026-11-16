use axum::{
    extract::{Path, Query, State},
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

#[derive(Deserialize)]
pub struct ExplainPayload {
    pub data: Vec<f64>,
    #[serde(default)]
    pub prediction_id: Option<Uuid>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/predict", post(predict))
        .route("/explain", post(explain))
        .route("/history/predictions", get(my_predictions))
        .route("/history/explanations", get(my_explanations))
        .route("/history/summary", get(my_summary))
        .route("/history/runs/{prediction_id}", get(get_run_detail))
}

pub(crate) fn normalize_tier(raw: &str) -> String {
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

#[derive(Debug, Clone, Copy)]
pub struct FeatureBound {
    pub name: &'static str,
    #[allow(dead_code)]
    pub p1: f64,
    #[allow(dead_code)]
    pub p99: f64,
    pub lo: f64,
    pub hi: f64,
}


impl FeatureBound {
    pub const fn new(name: &'static str, p1: f64, p99: f64) -> Self {
        let w = p99 - p1;
        let lo = p1 - 2.0 * w;
        let hi = p99 + 2.0 * w;
        Self {
            name,
            p1,
            p99,
            lo,
            hi,
        }
    }
}

pub const FEATURE_BOUNDS: [FeatureBound; 11] = [
    FeatureBound::new("qNet", 0.6429, 1.3713),
    FeatureBound::new("dvdtmax", 0.9279, 1.0012),
    FeatureBound::new("vmax", 0.9371, 1.0078),
    FeatureBound::new("vrest", -1.000361, -0.999964),
    FeatureBound::new("APD50", 0.9821, 1.9191),
    FeatureBound::new("APD90", 0.9966, 2.2129),
    FeatureBound::new("max_dv", -0.9935, -0.2593),
    FeatureBound::new("camax", 0.8389, 1.2591),
    FeatureBound::new("carest", 0.963, 1.020),
    FeatureBound::new("CaTD50", 0.9493, 1.1553),
    FeatureBound::new("CaTD90", 0.9831, 1.1767),
];

pub fn validate_features(data: &[f64]) -> Result<(), AppError> {
    if data.len() != 11 {
        return Err(AppError::Validation("Expected exactly 11 numeric features".into()));
    }

    if data.iter().any(|v| v.is_nan() || v.is_infinite()) {
        return Err(AppError::Validation("Feature values must not be NaN or infinite".into()));
    }

    for (val, bound) in data.iter().zip(FEATURE_BOUNDS.iter()) {
        if *val < bound.lo || *val > bound.hi {
            return Err(AppError::Validation(format!(
                "{} = {:.6} is outside the accepted range [{:.6}, {:.6}]. Inputs must be ratios to the drug-free control (see Table 3.1), not physical units.",
                bound.name, val, bound.lo, bound.hi
            )));
        }
    }

    Ok(())
}

async fn predict(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(payload): Json<PredictPayload>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    validate_features(&payload.data)?;

    let start = Instant::now();
    let data_val = serde_json::to_value(&payload.data).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    
    let result = call_gradio(&state.http_client, &state.config.hf_space_base, "predict", &data_val).await?;
    let elapsed = start.elapsed().as_millis() as i32;

    let predicted_tier = result
        .as_array()
        .and_then(|arr| {
            arr.first()
                .and_then(|v| v.get("label"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| arr.get(1).and_then(|v| v.as_str()))
        })
        .map(normalize_tier);
    
    let probabilities = result.as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("confidences"))
        .cloned();

    if probabilities.is_none() {
        tracing::debug!("Probabilities were null. Raw Gradio data array: {:?}", result);
    }

    let conformal_obj = result.as_array().and_then(|arr| arr.get(2));

    let prediction_set = conformal_obj.and_then(|obj| obj.get("prediction_set")).cloned();
    let recommended_action = conformal_obj
        .and_then(|obj| obj.get("recommended_action"))
        .and_then(|v| v.as_str())
        .map(normalize_tier);
    let is_ambiguous = conformal_obj
        .and_then(|obj| obj.get("is_ambiguous"))
        .and_then(|v| v.as_bool());
    let out_of_distribution = conformal_obj
        .and_then(|obj| obj.get("out_of_distribution"))
        .and_then(|v| v.as_bool());
    let alpha = conformal_obj
        .and_then(|obj| obj.get("alpha"))
        .and_then(|v| v.as_f64());
    let q_hat = conformal_obj
        .and_then(|obj| obj.get("q_hat"))
        .and_then(|v| v.as_f64());

    if conformal_obj.is_none() {
        tracing::debug!("Conformal fields were missing or malformed. Raw Gradio data array: {:?}", result);
    }

    let input_json = serde_json::to_value(&payload.data).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    let prediction_id = Uuid::new_v4();
    let db = state.db.clone();
    tokio::spawn(async move {
        let res = sqlx::query(
            r#"
            INSERT INTO prediction_logs (
                id, user_id, input, predicted_tier, probabilities, latency_ms,
                prediction_set, recommended_action, is_ambiguous, out_of_distribution, alpha, q_hat,
                source, job_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'digital_lab', NULL)
            "#,
        )
        .bind(prediction_id)
        .bind(user.id)
        .bind(&input_json)
        .bind(&predicted_tier)
        .bind(&probabilities)
        .bind(elapsed)
        .bind(&prediction_set)
        .bind(&recommended_action)
        .bind(is_ambiguous)
        .bind(out_of_distribution)
        .bind(alpha)
        .bind(q_hat)
        .execute(&db)
        .await;

        if let Err(e) = res {
            error!("Failed to record prediction log: {:?}", e);
        }
    });

    Ok(Json(serde_json::json!({
        "prediction_id": prediction_id,
        "data": result,
    })))
}

async fn explain(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(payload): Json<ExplainPayload>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    validate_features(&payload.data)?;

    let start = Instant::now();
    let data_val = serde_json::to_value(&payload.data).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    
    let result = call_gradio(&state.http_client, &state.config.hf_space_base, "explain", &data_val).await?;
    let elapsed = start.elapsed().as_millis() as i32;

    let predicted_class = result.as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("predicted_class"))
        .and_then(|v| v.as_str())
        .map(normalize_tier);

    let base_value = result.as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("base_value"))
        .and_then(|v| v.as_f64());

    let contributions = result.as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("contributions"))
        .cloned();

    let input_json = serde_json::to_value(&payload.data).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    let db = state.db.clone();
    let prediction_id = payload.prediction_id;
    tokio::spawn(async move {
        let res = sqlx::query(
            r#"
            INSERT INTO shap_logs (user_id, prediction_id, input, predicted_class, base_value, contributions, latency_ms)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(user.id)
        .bind(prediction_id)
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

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InputMethodDescriptor {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drug: Option<String>,
}

pub fn determine_input_method(
    source: &str,
    input_mode: Option<&str>,
    job_request: Option<&serde_json::Value>,
) -> InputMethodDescriptor {
    if source == "pipeline" {
        let mode = input_mode.map(str::to_string);
        let mode_label = match mode.as_deref() {
            Some("cipa_drug") => Some("CiPA drug".to_string()),
            Some("new_drug") => Some("New drug".to_string()),
            Some("cipa_drug_custom_data") => Some("CiPA drug + new lab data".to_string()),
            _ => None,
        };
        let drug = job_request
            .and_then(|r| r.get("drug"))
            .and_then(|d| d.as_str())
            .map(str::to_string);
        InputMethodDescriptor {
            method: "pipeline".to_string(),
            mode,
            mode_label,
            drug,
        }
    } else {
        InputMethodDescriptor {
            method: "digital_lab".to_string(),
            mode: None,
            mode_label: None,
            drug: None,
        }
    }
}

#[derive(Serialize, Clone)]
pub struct HistoryPredictionRow {
    pub id: Uuid,
    pub input: serde_json::Value,
    pub predicted_tier: Option<String>,
    pub probabilities: Option<serde_json::Value>,
    pub latency_ms: Option<i32>,
    pub prediction_set: Option<serde_json::Value>,
    pub recommended_action: Option<String>,
    pub is_ambiguous: Option<bool>,
    pub out_of_distribution: Option<bool>,
    pub alpha: Option<f64>,
    pub q_hat: Option<f64>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    pub source: String,
    pub job_id: Option<Uuid>,
    pub input_method: InputMethodDescriptor,
}

#[derive(sqlx::FromRow)]
struct DbPredictionRow {
    id: Uuid,
    input: serde_json::Value,
    predicted_tier: Option<String>,
    probabilities: Option<serde_json::Value>,
    latency_ms: Option<i32>,
    prediction_set: Option<serde_json::Value>,
    recommended_action: Option<String>,
    is_ambiguous: Option<bool>,
    out_of_distribution: Option<bool>,
    alpha: Option<f64>,
    q_hat: Option<f64>,
    created_at: OffsetDateTime,
    source: String,
    job_id: Option<Uuid>,
    job_input_mode: Option<String>,
    job_request: Option<serde_json::Value>,
}

impl DbPredictionRow {
    fn into_history_row(self) -> HistoryPredictionRow {
        let input_method = determine_input_method(
            &self.source,
            self.job_input_mode.as_deref(),
            self.job_request.as_ref(),
        );
        HistoryPredictionRow {
            id: self.id,
            input: self.input,
            predicted_tier: self.predicted_tier,
            probabilities: self.probabilities,
            latency_ms: self.latency_ms,
            prediction_set: self.prediction_set,
            recommended_action: self.recommended_action,
            is_ambiguous: self.is_ambiguous,
            out_of_distribution: self.out_of_distribution,
            alpha: self.alpha,
            q_hat: self.q_hat,
            created_at: self.created_at,
            source: self.source,
            job_id: self.job_id,
            input_method,
        }
    }
}

async fn my_predictions(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Query(filter): Query<HistoryFilter>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;
    let limit = filter.pagination.limit();
    let offset = filter.pagination.offset();

    let db_rows = sqlx::query_as::<_, DbPredictionRow>(
        r#"
        SELECT 
            p.id, p.input, p.predicted_tier, p.probabilities, p.latency_ms,
            p.prediction_set, p.recommended_action, p.is_ambiguous, p.out_of_distribution,
            p.alpha, p.q_hat, p.created_at, p.source, p.job_id,
            sj.input_mode AS job_input_mode, sj.request AS job_request
        FROM prediction_logs p
        LEFT JOIN simulation_jobs sj ON p.job_id = sj.id
        WHERE p.user_id = $1 AND p.source != 'pipeline_dose'
        ORDER BY p.created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(user.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let items: Vec<HistoryPredictionRow> = db_rows.into_iter().map(|r| r.into_history_row()).collect();

    let total: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs WHERE user_id = $1 AND source != 'pipeline_dose'")
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
        r#"
        SELECT s.id, s.prediction_id, s.input, s.predicted_class, s.base_value, s.contributions, s.latency_ms, s.created_at
        FROM shap_logs s
        JOIN prediction_logs p ON s.prediction_id = p.id
        WHERE s.user_id = $1 AND p.source != 'pipeline_dose'
        ORDER BY s.created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(user.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let total: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*)
        FROM shap_logs s
        JOIN prediction_logs p ON s.prediction_id = p.id
        WHERE s.user_id = $1 AND p.source != 'pipeline_dose'
        "#
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(PaginatedResponse { items, limit, offset, total: total.0 }))
}

#[derive(Serialize)]
pub struct RetentionPolicyResponse {
    pub trace_retention_days: u32,
    pub trace_max_jobs_per_user: u32,
}

#[derive(Serialize)]
pub struct HistorySummaryResponse {
    pub total_predictions: i64,
    pub total_explanations: i64,
    pub tier_counts: serde_json::Value,
    pub traces_available_count: i64,
    pub retention_policy: RetentionPolicyResponse,
}

async fn my_summary(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    let pred_count: (i64,) = sqlx::query_as("SELECT count(*) FROM prediction_logs WHERE user_id = $1 AND source != 'pipeline_dose'")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;

    let expl_count: (i64,) = sqlx::query_as(
        r#"
        SELECT count(DISTINCT s.prediction_id)
        FROM shap_logs s
        JOIN prediction_logs p ON s.prediction_id = p.id
        WHERE s.user_id = $1 AND p.source != 'pipeline_dose'
        "#
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    let traces_available_count: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*)
        FROM simulation_jobs
        WHERE user_id = $1
          AND status = 'done'
          AND traces_evicted_at IS NULL
          AND sim_result IS NOT NULL
        "#,
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    #[derive(sqlx::FromRow)]
    struct TierCount {
        predicted_tier: Option<String>,
        count: i64,
    }
    
    let tier_rows = sqlx::query_as::<_, TierCount>("SELECT predicted_tier, count(*) as count FROM prediction_logs WHERE user_id = $1 AND source != 'pipeline_dose' GROUP BY predicted_tier")
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
        traces_available_count: traces_available_count.0,
        retention_policy: RetentionPolicyResponse {
            trace_retention_days: state.config.retention_trace_days,
            trace_max_jobs_per_user: state.config.retention_trace_max_jobs_per_user,
        },
    }))
}

#[derive(Deserialize)]
pub struct RunDetailQuery {
    pub include_traces: Option<bool>,
}

#[derive(Serialize)]
pub struct PipelineTraces {
    pub trace: Option<serde_json::Value>,
    pub control_trace: Option<serde_json::Value>,
    pub traces_by_dose: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct PipelineRunDetail {
    pub job_id: Uuid,
    pub status: String,
    pub input_mode: Option<String>,
    pub sim_params: Option<serde_json::Value>,
    pub ratios: Option<serde_json::Value>,
    pub raw_metrics: Option<serde_json::Value>,
    pub control_metrics: Option<serde_json::Value>,
    pub per_dose: Option<serde_json::Value>,
    pub doses: Option<serde_json::Value>,
    pub dose_mode: Option<serde_json::Value>,
    pub sim_latency_ms: Option<i32>,
    pub traces_evicted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::iso8601::option")]
    pub traces_evicted_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traces: Option<PipelineTraces>,
}

#[derive(Serialize)]
pub struct RunDetailResponse {
    pub prediction: HistoryPredictionRow,
    pub explanation: Option<HistoryExplanationRow>,
    pub pipeline: Option<PipelineRunDetail>,
}

async fn get_run_detail(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(prediction_id): Path<Uuid>,
    Query(query): Query<RunDetailQuery>,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;
    let is_admin = user.role == "admin";

    #[derive(sqlx::FromRow)]
    struct RunDbPredictionRow {
        id: Uuid,
        user_id: Option<Uuid>,
        input: serde_json::Value,
        predicted_tier: Option<String>,
        probabilities: Option<serde_json::Value>,
        latency_ms: Option<i32>,
        prediction_set: Option<serde_json::Value>,
        recommended_action: Option<String>,
        is_ambiguous: Option<bool>,
        out_of_distribution: Option<bool>,
        alpha: Option<f64>,
        q_hat: Option<f64>,
        created_at: OffsetDateTime,
        source: String,
        job_id: Option<Uuid>,
        job_input_mode: Option<String>,
        job_request: Option<serde_json::Value>,
    }

    let pred_row: Option<RunDbPredictionRow> = sqlx::query_as(
        r#"
        SELECT 
            p.id, p.user_id, p.input, p.predicted_tier, p.probabilities, p.latency_ms,
            p.prediction_set, p.recommended_action, p.is_ambiguous, p.out_of_distribution,
            p.alpha, p.q_hat, p.created_at, p.source, p.job_id,
            sj.input_mode AS job_input_mode, sj.request AS job_request
        FROM prediction_logs p
        LEFT JOIN simulation_jobs sj ON p.job_id = sj.id
        WHERE p.id = $1
        "#,
    )
    .bind(prediction_id)
    .fetch_optional(&state.db)
    .await?;

    let pred_row = match pred_row {
        Some(r) => r,
        None => return Err(AppError::NotFound("Run not found".into())),
    };

    if !is_admin && pred_row.user_id != Some(user.id) {
        return Err(AppError::NotFound("Run not found".into()));
    }

    let input_method = determine_input_method(
        &pred_row.source,
        pred_row.job_input_mode.as_deref(),
        pred_row.job_request.as_ref(),
    );

    let prediction = HistoryPredictionRow {
        id: pred_row.id,
        input: pred_row.input,
        predicted_tier: pred_row.predicted_tier,
        probabilities: pred_row.probabilities,
        latency_ms: pred_row.latency_ms,
        prediction_set: pred_row.prediction_set,
        recommended_action: pred_row.recommended_action,
        is_ambiguous: pred_row.is_ambiguous,
        out_of_distribution: pred_row.out_of_distribution,
        alpha: pred_row.alpha,
        q_hat: pred_row.q_hat,
        created_at: pred_row.created_at,
        source: pred_row.source.clone(),
        job_id: pred_row.job_id,
        input_method,
    };

    let explanation: Option<HistoryExplanationRow> = sqlx::query_as(
        r#"
        SELECT id, prediction_id, input, predicted_class, base_value, contributions, latency_ms, created_at
        FROM shap_logs
        WHERE prediction_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(prediction_id)
    .fetch_optional(&state.db)
    .await?;

    let include_traces = query.include_traces.unwrap_or(false);
    let pipeline = if let Some(job_id) = pred_row.job_id {
        #[derive(sqlx::FromRow)]
        struct SimJobData {
            id: Uuid,
            status: String,
            input_mode: Option<String>,
            sim_params: Option<serde_json::Value>,
            ratios: Option<serde_json::Value>,
            sim_result: Option<serde_json::Value>,
            sim_latency_ms: Option<i32>,
            traces_evicted_at: Option<OffsetDateTime>,
        }

        let job_data: Option<SimJobData> = sqlx::query_as(
            r#"
            SELECT id, status, input_mode, sim_params, ratios, sim_result, sim_latency_ms, traces_evicted_at
            FROM simulation_jobs
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&state.db)
        .await?;

        job_data.map(|j| {
            let res = j.sim_result.as_ref();
            let per_dose = res.and_then(|r| r.get("per_dose")).cloned();
            let raw_metrics = res.and_then(|r| r.get("raw_metrics")).cloned();
            let control_metrics = res.and_then(|r| r.get("control_metrics")).cloned();
            let doses = res.and_then(|r| r.get("doses")).cloned();
            let dose_mode = res.and_then(|r| r.get("dose_mode")).cloned();
            let traces_evicted = j.traces_evicted_at.is_some();

            let traces = if include_traces && !traces_evicted {
                res.map(|r| PipelineTraces {
                    trace: r.get("trace").cloned(),
                    control_trace: r.get("control_trace").cloned(),
                    traces_by_dose: r.get("traces_by_dose").cloned(),
                })
            } else {
                None
            };

            PipelineRunDetail {
                job_id: j.id,
                status: j.status,
                input_mode: j.input_mode,
                sim_params: j.sim_params,
                ratios: j.ratios,
                raw_metrics,
                control_metrics,
                per_dose,
                doses,
                dose_mode,
                sim_latency_ms: j.sim_latency_ms,
                traces_evicted,
                traces_evicted_at: j.traces_evicted_at,
                traces,
            }
        })
    } else {
        None
    };

    Ok(Json(RunDetailResponse {
        prediction,
        explanation,
        pipeline,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_range_validation() {
        let test_vectors: [(&str, [f64; 11]); 14] = [
            ("Azimilide", [0.62631, 0.99975, 1.003687, -0.999976, 1.422269, 1.521552, -0.631328, 1.063894, 1.02, 1.121262, 1.056818]),
            ("Vandetanib", [0.518977, 0.998022, 0.99734, -1.000036, 1.616597, 1.775862, -0.472144, 1.02015, 1.0, 1.225083, 1.126033]),
            ("Disopyramide", [0.866951, 0.996498, 0.990098, -1.000025, 1.240546, 1.305172, -0.745638, 1.049315, 1.01, 1.060631, 1.02135]),
            ("Ibutilide", [0.328534, 0.992545, 1.021056, -1.000187, 1.994748, 2.303448, -0.235329, 1.019505, 0.992, 1.240864, 1.277893]),
            ("Clarithromycin", [0.918296, 0.999048, 0.990152, -1.000058, 1.115546, 1.14569, -0.838632, 0.937907, 0.99, 1.080565, 1.038912]),
            ("Domperidone", [0.6731, 1.000821, 1.00564, -1.000038, 1.385504, 1.435345, -0.671481, 1.046307, 1.01, 1.120432, 1.049242]),
            ("Clozapine", [0.957191, 0.999981, 0.999269, -1.000005, 1.064076, 1.076724, -0.913798, 1.011336, 1.0, 1.017442, 1.00551]),
            ("Droperidol", [0.876561, 1.00005, 1.001491, -0.999996, 1.139706, 1.157759, -0.841512, 1.028294, 1.01, 1.037375, 1.011708]),
            ("Loratadine", [0.999578, 1.0, 1.0, -1.0, 1.0, 1.0, -0.998329, 1.000133, 1.0, 1.0, 1.0]),
            ("Nifedipine", [1.188624, 1.000936, 0.965735, -1.000383, 0.921218, 0.932759, -0.986335, 0.713658, 0.916, 1.097176, 1.0823]),
            ("Tamoxifen", [0.985585, 0.999987, 1.000175, -0.999998, 1.013655, 1.016379, -0.97662, 1.003312, 1.0, 1.003322, 1.001377]),
            ("Nitrendipine", [1.03339, 1.000121, 0.996312, -1.00004, 0.985294, 0.987931, -0.998975, 0.96478, 0.993, 1.01412, 1.009642]),
            ("Table 3.1 Azimilide", [0.62631, 0.99975, 1.003687, -0.999976, 1.422269, 1.521552, -0.631328, 1.063894, 1.02, 1.121262, 1.056818]),
            ("Table 3.1 Clarithromycin", [0.918296, 0.999048, 0.990152, -1.000058, 1.115546, 1.14569, -0.838632, 0.937907, 0.99, 1.080565, 1.038912]),
        ];

        // (a) all 14 vectors pass
        for (drug, vec) in &test_vectors {
            let res = validate_features(vec);
            assert!(res.is_ok(), "Expected {} to pass range validation, but got: {:?}", drug, res.err());
        }

        // (b) physical-unit vector (APD90 = 338.0, vrest = -88.0) fails
        let mut phys_vec = test_vectors[0].1;
        phys_vec[3] = -88.0; // vrest
        phys_vec[5] = 338.0; // APD90
        let phys_res = validate_features(&phys_vec);
        assert!(phys_res.is_err(), "Expected physical-unit vector to fail range validation");
        match phys_res {
            Err(AppError::Validation(msg)) => {
                assert!(
                    msg.contains("outside the accepted range"),
                    "Expected 'outside the accepted range' in error message, got: {}",
                    msg
                );
                assert!(
                    msg.contains("Inputs must be ratios to the drug-free control (see Table 3.1), not physical units."),
                    "Expected standard guidance in error message, got: {}",
                    msg
                );
            }
            other => panic!("Expected AppError::Validation, got {:?}", other),
        }

        // (c) NaN fails
        let mut nan_vec = test_vectors[0].1;
        nan_vec[0] = f64::NAN;
        let nan_res = validate_features(&nan_vec);
        assert!(nan_res.is_err(), "Expected NaN vector to fail validation");
        match nan_res {
            Err(AppError::Validation(msg)) => {
                assert_eq!(msg, "Feature values must not be NaN or infinite");
            }
            other => panic!("Expected AppError::Validation, got {:?}", other),
        }
    }

    #[test]
    fn test_determine_input_method() {
        // Digital Lab
        let dl = determine_input_method("digital_lab", None, None);
        assert_eq!(dl.method, "digital_lab");
        assert_eq!(dl.mode, None);
        assert_eq!(dl.drug, None);

        // Pipeline Mode 1
        let req1 = serde_json::json!({ "drug": "cisapride" });
        let p1 = determine_input_method("pipeline", Some("cipa_drug"), Some(&req1));
        assert_eq!(p1.method, "pipeline");
        assert_eq!(p1.mode.as_deref(), Some("cipa_drug"));
        assert_eq!(p1.mode_label.as_deref(), Some("CiPA drug"));
        assert_eq!(p1.drug.as_deref(), Some("cisapride"));

        // Pipeline Mode 2
        let req2 = serde_json::json!({ "cmax_nM": 10.0, "herg": {} });
        let p2 = determine_input_method("pipeline", Some("new_drug"), Some(&req2));
        assert_eq!(p2.method, "pipeline");
        assert_eq!(p2.mode.as_deref(), Some("new_drug"));
        assert_eq!(p2.mode_label.as_deref(), Some("New drug"));
        assert_eq!(p2.drug, None);

        // Pipeline Mode 3
        let req3 = serde_json::json!({ "drug": "bepridil", "herg": {} });
        let p3 = determine_input_method("pipeline", Some("cipa_drug_custom_data"), Some(&req3));
        assert_eq!(p3.method, "pipeline");
        assert_eq!(p3.mode.as_deref(), Some("cipa_drug_custom_data"));
        assert_eq!(p3.mode_label.as_deref(), Some("CiPA drug + new lab data"));
        assert_eq!(p3.drug.as_deref(), Some("bepridil"));
    }

    #[test]
    fn test_run_detail_response_serialization() {
        let input_method = determine_input_method("digital_lab", None, None);
        let pred = HistoryPredictionRow {
            id: Uuid::nil(),
            input: serde_json::json!([1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
            predicted_tier: Some("Low".to_string()),
            probabilities: Some(serde_json::json!([{"confidence": 0.9, "label": "low"}])),
            latency_ms: Some(100),
            prediction_set: Some(serde_json::json!(["low"])),
            recommended_action: Some("Low".to_string()),
            is_ambiguous: Some(false),
            out_of_distribution: Some(false),
            alpha: Some(0.1),
            q_hat: Some(0.5),
            created_at: OffsetDateTime::UNIX_EPOCH,
            source: "digital_lab".to_string(),
            job_id: None,
            input_method,
        };

        let run_dl = RunDetailResponse {
            prediction: pred.clone(),
            explanation: None,
            pipeline: None,
        };
        let val_dl = serde_json::to_value(&run_dl).unwrap();
        assert_eq!(val_dl["prediction"]["source"], "digital_lab");
        assert!(val_dl["explanation"].is_null());
        assert!(val_dl["pipeline"].is_null());

        let pipeline_detail_no_traces = PipelineRunDetail {
            job_id: Uuid::nil(),
            status: "done".to_string(),
            input_mode: Some("cipa_drug".to_string()),
            sim_params: None,
            ratios: None,
            raw_metrics: Some(serde_json::json!({"qNet": 68.0})),
            control_metrics: Some(serde_json::json!({"qNet": 70.0})),
            per_dose: None,
            doses: None,
            dose_mode: None,
            sim_latency_ms: Some(200),
            traces_evicted: false,
            traces_evicted_at: None,
            traces: None,
        };
        let val_pipe = serde_json::to_value(&pipeline_detail_no_traces).unwrap();
        assert!(!val_pipe.as_object().unwrap().contains_key("traces"));
        assert_eq!(val_pipe["traces_evicted"], false);
        assert_eq!(val_pipe["control_metrics"]["qNet"], 70.0);
        assert_eq!(val_pipe["raw_metrics"]["qNet"], 68.0);
    }
}


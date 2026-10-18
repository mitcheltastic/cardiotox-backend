use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use time::OffsetDateTime;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::Instant;
use uuid::Uuid;

use crate::{
    auth::backend::AuthSession, error::AppError, services::prediction::call_gradio, state::AppState,
};

static SIM_SEMAPHORE: Semaphore = Semaphore::const_new(1);
static DRUGS_CACHE: RwLock<Option<(Instant, Value)>> = RwLock::const_new(None);

#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct HergParams {
    #[serde(default)]
    pub Kmax: Option<f64>,
    #[serde(default)]
    pub Ku: Option<f64>,
    #[serde(default)]
    pub n: Option<f64>,
    #[serde(default)]
    pub halfmax: Option<f64>,
    #[serde(default)]
    pub Vhalf: Option<f64>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Ic50Params {
    #[serde(default)]
    pub INa_IC50: Option<f64>,
    #[serde(default)]
    pub INa_h: Option<f64>,
    #[serde(default)]
    pub INaL_IC50: Option<f64>,
    #[serde(default)]
    pub INaL_h: Option<f64>,
    #[serde(default)]
    pub ICaL_IC50: Option<f64>,
    #[serde(default)]
    pub ICaL_h: Option<f64>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct PipelineJobPayload {
    #[serde(default)]
    pub drug: Option<String>,
    #[serde(default)]
    pub cmax_nM: Option<f64>,
    #[serde(default)]
    pub dose: Option<f64>,
    #[serde(default)]
    pub herg: Option<HergParams>,
    #[serde(default)]
    pub ic50: Option<Ic50Params>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActiveJobDetailsResponse {
    pub id: Uuid,
    pub status: String,
    pub request: Value,
    pub error: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub finished_at: Option<OffsetDateTime>,
    pub sim_params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompleteJobDetailsResponse {
    pub id: Uuid,
    pub status: String,
    pub request: Value,
    pub error: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub finished_at: Option<OffsetDateTime>,
    pub sim_params: Option<Value>,
    pub ratios: Option<Value>,
    pub sim_result: Option<Value>,
    pub prediction_id: Option<Uuid>,
    pub prediction: Option<Value>,
    pub explanation: Option<Value>,
    pub sim_latency_ms: Option<i32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum JobDetailsResponse {
    Active(ActiveJobDetailsResponse),
    Complete(CompleteJobDetailsResponse),
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SimulationJobRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub status: String,
    pub request: Value,
    pub n_samples: i32,
    pub sim_params: Option<Value>,
    pub ratios: Option<Value>,
    pub sim_result: Option<Value>,
    pub prediction_id: Option<Uuid>,
    pub prediction: Option<Value>,
    pub explanation: Option<Value>,
    pub error: Option<String>,
    pub sim_latency_ms: Option<i32>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub finished_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LightSimulationJobRow {
    pub id: Uuid,
    pub status: String,
    pub request: Value,
    pub error: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub finished_at: Option<OffsetDateTime>,
    pub prediction_id: Option<Uuid>,
}

pub async fn reset_stale_jobs(db: &sqlx::PgPool) {
    if let Err(e) = sqlx::query(
        "UPDATE simulation_jobs SET status='failed', error='Interrupted by server restart', finished_at=now() WHERE status IN ('queued','simulating','predicting')"
    )
    .execute(db)
    .await
    {
        tracing::warn!("could not reset stale simulation jobs: {e}");
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pipeline/jobs", post(create_job).get(list_jobs))
        .route("/pipeline/jobs/{id}", get(get_job))
        .route("/pipeline/drugs", get(get_drugs))
}

pub fn validate_pipeline_request(payload: &PipelineJobPayload) -> Result<(), AppError> {
    let has_valid_drug = if let Some(drug) = &payload.drug {
        if drug.is_empty()
            || drug.len() > 40
            || !drug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(AppError::Validation(
                "Drug name must be lowercase, alphanumeric or hyphen only, and max 40 characters"
                    .into(),
            ));
        }
        true
    } else {
        false
    };

    let has_full_herg = if let (Some(cmax), Some(herg)) = (payload.cmax_nM, &payload.herg) {
        if !cmax.is_finite() || cmax <= 0.0 {
            return Err(AppError::Validation(
                "cmax_nM must be a positive finite number".into(),
            ));
        }
        let kmax = herg
            .Kmax
            .ok_or_else(|| AppError::Validation("Missing herg.Kmax".into()))?;
        let ku = herg
            .Ku
            .ok_or_else(|| AppError::Validation("Missing herg.Ku".into()))?;
        let n = herg
            .n
            .ok_or_else(|| AppError::Validation("Missing herg.n".into()))?;
        let halfmax = herg
            .halfmax
            .ok_or_else(|| AppError::Validation("Missing herg.halfmax".into()))?;
        let vhalf = herg
            .Vhalf
            .ok_or_else(|| AppError::Validation("Missing herg.Vhalf".into()))?;

        if !kmax.is_finite() || kmax <= 0.0 {
            return Err(AppError::Validation(
                "herg.Kmax must be a positive finite number".into(),
            ));
        }
        if !ku.is_finite() || ku <= 0.0 {
            return Err(AppError::Validation(
                "herg.Ku must be a positive finite number".into(),
            ));
        }
        if !n.is_finite() || n <= 0.0 {
            return Err(AppError::Validation(
                "herg.n must be a positive finite number".into(),
            ));
        }
        if !halfmax.is_finite() || halfmax <= 0.0 {
            return Err(AppError::Validation(
                "herg.halfmax must be a positive finite number".into(),
            ));
        }
        if !vhalf.is_finite() {
            return Err(AppError::Validation(
                "herg.Vhalf must be a finite number".into(),
            ));
        }
        true
    } else {
        false
    };

    if !has_valid_drug && !has_full_herg {
        return Err(AppError::Validation(
            "Request must specify either 'drug' or both 'cmax_nM' and all 5 herg parameters (Kmax, Ku, n, halfmax, Vhalf)".into(),
        ));
    }

    if let Some(cmax) = payload.cmax_nM
        && (!cmax.is_finite() || cmax <= 0.0)
    {
        return Err(AppError::Validation(
            "cmax_nM must be a positive finite number".into(),
        ));
    }

    if let Some(herg) = &payload.herg {
        if let Some(v) = herg.Kmax
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(AppError::Validation(
                "herg.Kmax must be a positive finite number".into(),
            ));
        }
        if let Some(v) = herg.Ku
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(AppError::Validation(
                "herg.Ku must be a positive finite number".into(),
            ));
        }
        if let Some(v) = herg.n
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(AppError::Validation(
                "herg.n must be a positive finite number".into(),
            ));
        }
        if let Some(v) = herg.halfmax
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(AppError::Validation(
                "herg.halfmax must be a positive finite number".into(),
            ));
        }
        if let Some(v) = herg.Vhalf
            && !v.is_finite()
        {
            return Err(AppError::Validation(
                "herg.Vhalf must be a finite number".into(),
            ));
        }
    }

    if let Some(dose) = payload.dose
        && (!dose.is_finite() || dose <= 0.0 || dose > 100.0)
    {
        return Err(AppError::Validation(
            "dose must be a finite number in the range (0, 100]".into(),
        ));
    }

    if let Some(ic50) = &payload.ic50 {
        let fields = [
            ("INa_IC50", ic50.INa_IC50),
            ("INa_h", ic50.INa_h),
            ("INaL_IC50", ic50.INaL_IC50),
            ("INaL_h", ic50.INaL_h),
            ("ICaL_IC50", ic50.ICaL_IC50),
            ("ICaL_h", ic50.ICaL_h),
        ];
        for (name, opt) in fields {
            if let Some(val) = opt
                && (!val.is_finite() || val <= 0.0)
            {
                return Err(AppError::Validation(format!(
                    "{} must be a positive finite number",
                    name
                )));
            }
        }
    }

    Ok(())
}

async fn create_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Json(payload): Json<PipelineJobPayload>,
) -> Result<Response, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    if state.config.sim_api_key.is_none() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Full pipeline is not configured" })),
        )
            .into_response());
    }

    validate_pipeline_request(&payload)?;

    let active_job: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM simulation_jobs
        WHERE user_id = $1
          AND status IN ('queued', 'simulating', 'predicting')
          AND created_at > now() - interval '10 minutes'
        LIMIT 1
        "#,
    )
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?;

    if active_job.is_some() {
        return Ok((
            StatusCode::CONFLICT,
            Json(json!({ "error": "You already have a simulation running" })),
        )
            .into_response());
    }

    let req_val =
        serde_json::to_value(&payload).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    let (job_id,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO simulation_jobs (user_id, status, request)
        VALUES ($1, 'queued', $2)
        RETURNING id
        "#,
    )
    .bind(user.id)
    .bind(&req_val)
    .fetch_one(&state.db)
    .await?;

    tokio::spawn(run_pipeline_job(job_id, user.id, state.clone()));

    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))).into_response())
}

async fn get_job(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    let job: Option<SimulationJobRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, status, request, n_samples, sim_params, ratios, sim_result,
               prediction_id, prediction, explanation, error, sim_latency_ms,
               created_at, started_at, finished_at
        FROM simulation_jobs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let job = job.ok_or_else(|| AppError::NotFound("Job not found".into()))?;

    if job.user_id != Some(user.id) {
        return Err(AppError::NotFound("Job not found".into()));
    }

    let is_active = matches!(job.status.as_str(), "queued" | "simulating" | "predicting");
    let response = if is_active {
        JobDetailsResponse::Active(ActiveJobDetailsResponse {
            id: job.id,
            status: job.status,
            request: job.request,
            error: job.error,
            created_at: job.created_at,
            started_at: job.started_at,
            finished_at: job.finished_at,
            sim_params: job.sim_params,
        })
    } else {
        JobDetailsResponse::Complete(CompleteJobDetailsResponse {
            id: job.id,
            status: job.status,
            request: job.request,
            error: job.error,
            created_at: job.created_at,
            started_at: job.started_at,
            finished_at: job.finished_at,
            sim_params: job.sim_params,
            ratios: job.ratios,
            sim_result: job.sim_result,
            prediction_id: job.prediction_id,
            prediction: job.prediction,
            explanation: job.explanation,
            sim_latency_ms: job.sim_latency_ms,
        })
    };

    Ok(Json(response).into_response())
}

async fn list_jobs(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, AppError> {
    let user = auth_session.user.ok_or(AppError::Unauthorized)?;

    let jobs: Vec<LightSimulationJobRow> = sqlx::query_as(
        r#"
        SELECT id, status, request, error, created_at, finished_at, prediction_id
        FROM simulation_jobs
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(jobs).into_response())
}

async fn get_drugs(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, AppError> {
    let _user = auth_session.user.ok_or(AppError::Unauthorized)?;

    if state.config.sim_api_key.is_none() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Full pipeline is not configured" })),
        )
            .into_response());
    }

    {
        let read = DRUGS_CACHE.read().await;
        if let Some((cached_at, val)) = read.as_ref()
            && cached_at.elapsed() < Duration::from_secs(3600)
        {
            return Ok(Json(val.clone()).into_response());
        }
    }

    let sim_key = state.config.sim_api_key.as_deref().unwrap();
    let res = call_gradio(
        &state.http_client,
        &state.config.hf_space_base,
        "sim_drugs",
        &json!([sim_key]),
    )
    .await?;

    let payload = res
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(res);

    {
        let mut write = DRUGS_CACHE.write().await;
        *write = Some((Instant::now(), payload.clone()));
    }

    Ok(Json(payload).into_response())
}

pub fn parse_simulate_response(result: &Value) -> Result<(Value, Value, Value), String> {
    let payload = result
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or_else(|| "Empty simulation response array".to_string())?;

    let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        let err_msg = payload
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Simulation failed");
        return Err(err_msg.to_string());
    }

    let sim_params = payload
        .get("params")
        .cloned()
        .ok_or_else(|| "Simulation response missing 'params'".to_string())?;

    let ratios = payload
        .get("ratios")
        .cloned()
        .ok_or_else(|| "Simulation response missing 'ratios'".to_string())?;

    let mut result_obj = payload.as_object().cloned().unwrap_or_default();
    result_obj.remove("ratios");
    result_obj.remove("params");
    let sim_result = Value::Object(result_obj);

    Ok((sim_params, ratios, sim_result))
}

pub fn validate_ratios(ratios_val: &Value) -> Result<Vec<f64>, String> {
    let arr = ratios_val
        .as_array()
        .ok_or_else(|| "ratios must be an array".to_string())?;

    if arr.len() != 11 {
        return Err(format!("Expected 11 ratios, got {}", arr.len()));
    }

    let mut res = Vec::with_capacity(11);
    for (i, v) in arr.iter().enumerate() {
        let num = v
            .as_f64()
            .ok_or_else(|| format!("ratio[{}] is not a number", i))?;
        if !num.is_finite() {
            return Err(format!("ratio[{}] is not finite", i));
        }
        res.push(num);
    }

    Ok(res)
}

fn sanitize_error(msg: &str, sim_key: Option<&str>) -> String {
    let mut clean = msg.to_string();
    if let Some(key) = sim_key
        && !key.is_empty()
    {
        clean = clean.replace(key, "[REDACTED]");
    }
    if clean.chars().count() > 300 {
        clean = clean.chars().take(300).collect::<String>() + "...";
    }
    clean
}

async fn mark_job_failed(db: &sqlx::PgPool, job_id: Uuid, error_msg: &str) {
    tracing::error!("Pipeline job {} failed: {}", job_id, error_msg);
    let _ = sqlx::query(
        r#"
        UPDATE simulation_jobs
        SET status = 'failed',
            error = $2,
            finished_at = now()
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .bind(error_msg)
    .execute(db)
    .await;
}

async fn run_pipeline_job(job_id: Uuid, user_id: Uuid, state: AppState) {
    let _permit = match SIM_SEMAPHORE.acquire().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to acquire simulation semaphore: {:?}", e);
            mark_job_failed(&state.db, job_id, "Internal simulation queue error").await;
            return;
        }
    };

    let job_opt: Option<(Value,)> =
        match sqlx::query_as("SELECT request FROM simulation_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Failed to fetch job {} request: {:?}", job_id, e);
                mark_job_failed(&state.db, job_id, "Database error").await;
                return;
            }
        };

    let request_val = match job_opt {
        Some((r,)) => r,
        None => {
            tracing::error!("Job {} not found in database", job_id);
            return;
        }
    };

    let sim_api_key = match state.config.sim_api_key.as_deref() {
        Some(k) => k,
        None => {
            mark_job_failed(&state.db, job_id, "Simulation API key is not configured").await;
            return;
        }
    };

    if let Err(e) = sqlx::query(
        "UPDATE simulation_jobs SET status = 'simulating', started_at = now() WHERE id = $1",
    )
    .bind(job_id)
    .execute(&state.db)
    .await
    {
        let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
        mark_job_failed(&state.db, job_id, &safe_err).await;
        return;
    }

    let request_json_string = match serde_json::to_string(&request_val) {
        Ok(s) => s,
        Err(e) => {
            mark_job_failed(
                &state.db,
                job_id,
                &format!("Failed to serialize request: {}", e),
            )
            .await;
            return;
        }
    };

    let sim_data = json!([request_json_string, sim_api_key]);
    let sim_start = Instant::now();

    let sim_res_result = tokio::time::timeout(
        Duration::from_secs(240),
        call_gradio(
            &state.http_client,
            &state.config.hf_space_base,
            "simulate",
            &sim_data,
        ),
    )
    .await;

    let sim_res = match sim_res_result {
        Ok(Ok(val)) => val,
        Ok(Err(e)) => {
            let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
            tracing::error!("Gradio 'simulate' failed for job {}: {}", job_id, safe_err);
            mark_job_failed(&state.db, job_id, &safe_err).await;
            return;
        }
        Err(_) => {
            mark_job_failed(&state.db, job_id, "Simulation timed out after 240 seconds").await;
            return;
        }
    };
    let sim_latency_ms = sim_start.elapsed().as_millis() as i32;

    let (sim_params, ratios, sim_result) = match parse_simulate_response(&sim_res) {
        Ok(parsed) => parsed,
        Err(e) => {
            let safe_err = sanitize_error(&e, Some(sim_api_key));
            mark_job_failed(&state.db, job_id, &safe_err).await;
            return;
        }
    };

    let ratios_vec = match validate_ratios(&ratios) {
        Ok(v) => v,
        Err(e) => {
            mark_job_failed(
                &state.db,
                job_id,
                &format!("Invalid simulation ratios: {}", e),
            )
            .await;
            return;
        }
    };

    let update_pred_status = sqlx::query(
        r#"
        UPDATE simulation_jobs
        SET status = 'predicting',
            sim_params = $2,
            ratios = $3,
            sim_result = $4,
            sim_latency_ms = $5
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .bind(&sim_params)
    .bind(&ratios)
    .bind(&sim_result)
    .bind(sim_latency_ms)
    .execute(&state.db)
    .await;

    if let Err(e) = update_pred_status {
        let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
        mark_job_failed(&state.db, job_id, &safe_err).await;
        return;
    }

    let ratios_val = Value::Array(ratios_vec.iter().map(|&x| json!(x)).collect());
    let pred_start = Instant::now();

    let pred_res = match call_gradio(
        &state.http_client,
        &state.config.hf_space_base,
        "predict",
        &ratios_val,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
            tracing::error!("Gradio 'predict' failed for job {}: {}", job_id, safe_err);
            mark_job_failed(&state.db, job_id, &safe_err).await;
            return;
        }
    };
    let pred_latency_ms = pred_start.elapsed().as_millis() as i32;

    let predicted_tier = pred_res
        .as_array()
        .and_then(|arr| {
            arr.first()
                .and_then(|v| v.get("label"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| arr.get(1).and_then(|v| v.as_str()))
        })
        .map(crate::api::normalize_tier);

    let probabilities = pred_res
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("confidences"))
        .cloned();

    let conformal_obj = pred_res.as_array().and_then(|arr| arr.get(2));
    let prediction_set = conformal_obj
        .and_then(|obj| obj.get("prediction_set"))
        .cloned();
    let recommended_action = conformal_obj
        .and_then(|obj| obj.get("recommended_action"))
        .and_then(|v| v.as_str())
        .map(crate::api::normalize_tier);
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

    let prediction_id = Uuid::new_v4();
    if let Err(e) = sqlx::query(
        r#"
        INSERT INTO prediction_logs (
            id, user_id, input, predicted_tier, probabilities, latency_ms,
            prediction_set, recommended_action, is_ambiguous, out_of_distribution, alpha, q_hat
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(prediction_id)
    .bind(user_id)
    .bind(&ratios_val)
    .bind(&predicted_tier)
    .bind(&probabilities)
    .bind(pred_latency_ms)
    .bind(&prediction_set)
    .bind(&recommended_action)
    .bind(is_ambiguous)
    .bind(out_of_distribution)
    .bind(alpha)
    .bind(q_hat)
    .execute(&state.db)
    .await
    {
        let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
        mark_job_failed(&state.db, job_id, &safe_err).await;
        return;
    }

    let expl_start = Instant::now();
    let expl_res = match call_gradio(
        &state.http_client,
        &state.config.hf_space_base,
        "explain",
        &ratios_val,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
            tracing::error!("Gradio 'explain' failed for job {}: {}", job_id, safe_err);
            mark_job_failed(&state.db, job_id, &safe_err).await;
            return;
        }
    };
    let expl_latency_ms = expl_start.elapsed().as_millis() as i32;

    let predicted_class = expl_res
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("predicted_class"))
        .and_then(|v| v.as_str())
        .map(crate::api::normalize_tier);

    let base_value = expl_res
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("base_value"))
        .and_then(|v| v.as_f64());

    let contributions = expl_res
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("contributions"))
        .cloned();

    if let Err(e) = sqlx::query(
        r#"
        INSERT INTO shap_logs (user_id, prediction_id, input, predicted_class, base_value, contributions, latency_ms)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(user_id)
    .bind(Some(prediction_id))
    .bind(&ratios_val)
    .bind(&predicted_class)
    .bind(base_value)
    .bind(&contributions)
    .bind(expl_latency_ms)
    .execute(&state.db)
    .await
    {
        let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
        mark_job_failed(&state.db, job_id, &safe_err).await;
        return;
    }

    let done_res = sqlx::query(
        r#"
        UPDATE simulation_jobs
        SET status = 'done',
            prediction_id = $2,
            prediction = $3,
            explanation = $4,
            finished_at = now()
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .bind(prediction_id)
    .bind(&pred_res)
    .bind(&expl_res)
    .execute(&state.db)
    .await;

    if let Err(e) = done_res {
        let safe_err = sanitize_error(&e.to_string(), Some(sim_api_key));
        mark_job_failed(&state.db, job_id, &safe_err).await;
    } else {
        tracing::info!("Simulation job {} completed successfully", job_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_mode_1_drug_only() {
        let payload = PipelineJobPayload {
            drug: Some("cisapride".to_string()),
            dose: Some(1.0),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload).is_ok());

        let payload_no_dose = PipelineJobPayload {
            drug: Some("dofetilide-2".to_string()),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_no_dose).is_ok());
    }

    #[test]
    fn test_valid_mode_2_custom_params() {
        let payload = PipelineJobPayload {
            cmax_nM: Some(15.0),
            herg: Some(HergParams {
                Kmax: Some(1.2),
                Ku: Some(0.04),
                n: Some(1.0),
                halfmax: Some(30.0),
                Vhalf: Some(-10.5),
            }),
            ic50: Some(Ic50Params {
                INa_IC50: Some(500.0),
                INa_h: Some(1.0),
                ..Default::default()
            }),
            dose: Some(2.5),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload).is_ok());
    }

    #[test]
    fn test_valid_mode_3_drug_and_custom() {
        let payload = PipelineJobPayload {
            drug: Some("bepridil".to_string()),
            cmax_nM: Some(20.0),
            herg: Some(HergParams {
                Kmax: Some(2.0),
                Ku: Some(0.1),
                n: Some(1.0),
                halfmax: Some(40.0),
                Vhalf: Some(-5.0),
            }),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload).is_ok());
    }

    #[test]
    fn test_missing_herg() {
        // cmax provided without herg
        let payload_no_herg = PipelineJobPayload {
            cmax_nM: Some(10.0),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_no_herg).is_err());

        // herg missing halfmax
        let payload_incomplete_herg = PipelineJobPayload {
            cmax_nM: Some(10.0),
            herg: Some(HergParams {
                Kmax: Some(1.0),
                Ku: Some(0.1),
                n: Some(1.0),
                halfmax: None,
                Vhalf: Some(-10.0),
            }),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_incomplete_herg).is_err());

        // Neither drug nor cmax/herg provided
        let payload_empty = PipelineJobPayload::default();
        assert!(validate_pipeline_request(&payload_empty).is_err());
    }

    #[test]
    fn test_negative_vhalf_accepted() {
        let payload = PipelineJobPayload {
            cmax_nM: Some(10.0),
            herg: Some(HergParams {
                Kmax: Some(1.0),
                Ku: Some(0.1),
                n: Some(1.0),
                halfmax: Some(10.0),
                Vhalf: Some(-45.5),
            }),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload).is_ok());
    }

    #[test]
    fn test_ic50_less_equal_zero_rejected() {
        let payload_zero = PipelineJobPayload {
            drug: Some("quinidine".to_string()),
            ic50: Some(Ic50Params {
                INa_IC50: Some(0.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_zero).is_err());

        let payload_negative = PipelineJobPayload {
            drug: Some("quinidine".to_string()),
            ic50: Some(Ic50Params {
                ICaL_h: Some(-0.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_negative).is_err());
    }

    #[test]
    fn test_bad_drug_name() {
        // Uppercase
        let payload_upper = PipelineJobPayload {
            drug: Some("Cisapride".to_string()),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_upper).is_err());

        // Spaces
        let payload_space = PipelineJobPayload {
            drug: Some("cisa pride".to_string()),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_space).is_err());

        // Special characters
        let payload_special = PipelineJobPayload {
            drug: Some("cisa_pride!".to_string()),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_special).is_err());

        // Longer than 40 chars
        let long_drug = "a".repeat(41);
        let payload_long = PipelineJobPayload {
            drug: Some(long_drug),
            ..Default::default()
        };
        assert!(validate_pipeline_request(&payload_long).is_err());
    }

    #[test]
    fn test_parse_simulate_response_ok() {
        let fixture = json!([
            {
                "ok": true,
                "params": {
                    "drug": "cisapride",
                    "cmax_nM": 10.0
                },
                "ratios": [0.62, 0.99, 1.0, -1.0, 1.4, 1.5, -0.6, 1.05, 1.0, 1.1, 1.05],
                "raw_metrics": {"qNet": 0.5},
                "control_metrics": {"qNet": 0.8},
                "flags": [],
                "trace": [1.0, 2.0],
                "control_trace": [1.0, 2.1],
                "conc_nM": 10.0,
                "kept_beat": 100,
                "elapsed_s": 42.5,
                "engine_version": "1.0.0"
            }
        ]);

        let parsed = parse_simulate_response(&fixture);
        assert!(parsed.is_ok(), "Expected Ok but got: {:?}", parsed.err());
        let (params, ratios, sim_result) = parsed.unwrap();

        assert_eq!(params["drug"], "cisapride");
        let ratios_arr = ratios.as_array().unwrap();
        assert_eq!(ratios_arr.len(), 11);

        let res_obj = sim_result.as_object().unwrap();
        assert!(!res_obj.contains_key("ratios"));
        assert!(!res_obj.contains_key("params"));
        assert_eq!(res_obj["engine_version"], "1.0.0");
        assert_eq!(res_obj["elapsed_s"], 42.5);
        assert!(res_obj.contains_key("raw_metrics"));
    }

    #[test]
    fn test_parse_simulate_response_error() {
        let fixture = json!([
            {
                "ok": false,
                "error": "Simulation solver failed to converge"
            }
        ]);

        let parsed = parse_simulate_response(&fixture);
        assert!(parsed.is_err());
        assert_eq!(parsed.unwrap_err(), "Simulation solver failed to converge");
    }

    #[test]
    fn test_validate_ratios() {
        let valid_11 = json!([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
        let res = validate_ratios(&valid_11);
        assert!(res.is_ok());
        assert_eq!(res.unwrap().len(), 11);

        let invalid_len = json!([1.0, 2.0]);
        assert!(validate_ratios(&invalid_len).is_err());

        let invalid_nan = json!([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, null]);
        assert!(validate_ratios(&invalid_nan).is_err());
    }

    #[test]
    fn test_job_details_response_light_vs_heavy_serialization() {
        let now = OffsetDateTime::now_utc();
        let active_resp = JobDetailsResponse::Active(ActiveJobDetailsResponse {
            id: Uuid::nil(),
            status: "simulating".to_string(),
            request: json!({"drug": "cisapride"}),
            error: None,
            created_at: now,
            started_at: Some(now),
            finished_at: None,
            sim_params: Some(json!({"cmax_nM": 10.0})),
        });

        let active_json = serde_json::to_value(&active_resp).unwrap();
        let active_obj = active_json.as_object().unwrap();
        assert!(active_obj.contains_key("id"));
        assert!(active_obj.contains_key("status"));
        assert!(active_obj.contains_key("request"));
        assert!(active_obj.contains_key("error"));
        assert!(active_obj.contains_key("created_at"));
        assert!(active_obj.contains_key("started_at"));
        assert!(active_obj.contains_key("finished_at"));
        assert!(active_obj.contains_key("sim_params"));
        // Heavy fields must NOT be present:
        assert!(!active_obj.contains_key("ratios"));
        assert!(!active_obj.contains_key("sim_result"));
        assert!(!active_obj.contains_key("prediction_id"));
        assert!(!active_obj.contains_key("prediction"));
        assert!(!active_obj.contains_key("explanation"));
        assert!(!active_obj.contains_key("sim_latency_ms"));

        let complete_resp = JobDetailsResponse::Complete(CompleteJobDetailsResponse {
            id: Uuid::nil(),
            status: "done".to_string(),
            request: json!({"drug": "cisapride"}),
            error: None,
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            sim_params: Some(json!({"cmax_nM": 10.0})),
            ratios: Some(json!(vec![1.0; 11])),
            sim_result: Some(json!({"engine": "1.0"})),
            prediction_id: Some(Uuid::nil()),
            prediction: Some(json!([{"label": "Low"}])),
            explanation: Some(json!([{"predicted_class": "Low"}])),
            sim_latency_ms: Some(1500),
        });

        let complete_json = serde_json::to_value(&complete_resp).unwrap();
        let complete_obj = complete_json.as_object().unwrap();
        // Heavy fields MUST be present:
        assert!(complete_obj.contains_key("ratios"));
        assert!(complete_obj.contains_key("sim_result"));
        assert!(complete_obj.contains_key("prediction_id"));
        assert!(complete_obj.contains_key("prediction"));
        assert!(complete_obj.contains_key("explanation"));
        assert!(complete_obj.contains_key("sim_latency_ms"));
    }
}

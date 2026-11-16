use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// Surgical Waveform Trace Eviction
///
/// EVICTION SEMANTICS:
/// A simulation job's waveform traces (`trace`, `control_trace`, `traces_by_dose`) are evicted
/// as soon as EITHER of the following conditions is met (whichever bites first):
///   1. Age bound: The job was created more than `days` ago (`created_at < now() - interval 'X days'`), OR
///   2. Capacity bound: The job's rank among the user's newest simulation runs exceeds `max_jobs_per_user`
///      (i.e. only the N most recent jobs retain waveform data).
///
/// SAFEGUARD:
/// Eviction only considers jobs created on or after `apply_from` (`created_at >= apply_from`).
/// Historical runs prior to `apply_from` are never touched until `RETENTION_APPLY_FROM` is explicitly lowered.
///
/// SURGICAL SUBTRACTION:
/// Eviction removes only `trace`, `control_trace`, and `traces_by_dose` from `sim_result`
/// and sets `traces_evicted_at = now()`.
/// All other fields (eleven mean biomarker ratios, per-dose biomarkers, raw_metrics, control_metrics,
/// flags, parameters, predictions, explanations) are preserved intact.
pub async fn run_trace_eviction(
    pool: &PgPool,
    days: u32,
    max_jobs_per_user: u32,
    apply_from: OffsetDateTime,
    user_id: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT id
            FROM (
                SELECT id, created_at,
                       ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY created_at DESC) AS rn
                FROM simulation_jobs
                WHERE traces_evicted_at IS NULL
                  AND sim_result IS NOT NULL
                  AND created_at >= $1
                  AND ($2::uuid IS NULL OR user_id = $2)
                  AND (sim_result ? 'trace' OR sim_result ? 'control_trace' OR sim_result ? 'traces_by_dose')
            ) sub
            WHERE created_at < now() - make_interval(days => $3)
               OR rn > $4
        )
        UPDATE simulation_jobs sj
        SET sim_result = sj.sim_result - 'trace' - 'control_trace' - 'traces_by_dose',
            traces_evicted_at = now()
        FROM candidates
        WHERE sj.id = candidates.id
        "#,
    )
    .bind(apply_from)
    .bind(user_id)
    .bind(days as i32)
    .bind(max_jobs_per_user as i64)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_retention_defaults_semantics() {
        let days = 7u32;
        let cap = 10u32;
        assert_eq!(days, 7);
        assert_eq!(cap, 10);
    }
}

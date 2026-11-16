-- Migration 0011: Run-centric history, provenance, and input_mode

-- 1. Add provenance columns to prediction_logs
ALTER TABLE prediction_logs
    ADD COLUMN IF NOT EXISTS source text NOT NULL DEFAULT 'digital_lab',
    ADD COLUMN IF NOT EXISTS job_id uuid REFERENCES simulation_jobs(id) ON DELETE SET NULL;

-- 2. Add CHECK constraint on source allowing digital_lab, pipeline, and pipeline_dose
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'chk_prediction_logs_source'
    ) THEN
        ALTER TABLE prediction_logs
            ADD CONSTRAINT chk_prediction_logs_source
            CHECK (source IN ('digital_lab', 'pipeline', 'pipeline_dose'));
    END IF;
END $$;

-- 3. Add input_mode column to simulation_jobs
ALTER TABLE simulation_jobs
    ADD COLUMN IF NOT EXISTS input_mode text;

-- 4. Backfill input_mode on existing simulation_jobs
UPDATE simulation_jobs
SET input_mode = CASE 
    WHEN request ? 'drug' AND NOT (request ? 'herg' OR request ? 'ic50') THEN 'cipa_drug'
    WHEN NOT (request ? 'drug') AND (request ? 'herg') THEN 'new_drug'
    WHEN request ? 'drug' AND (request ? 'herg' OR request ? 'ic50') THEN 'cipa_drug_custom_data'
    ELSE 'cipa_drug'
END
WHERE input_mode IS NULL;

-- 5. Backfill run-level pipeline predictions (62 rows)
UPDATE prediction_logs p
SET source = 'pipeline',
    job_id = s.id
FROM simulation_jobs s
WHERE s.prediction_id = p.id
  AND (p.source != 'pipeline' OR p.job_id IS DISTINCT FROM s.id);

-- 6. Backfill legacy per-dose pipeline predictions (149 rows)
WITH sim_doses AS (
    SELECT 
        sj.id AS job_id,
        sj.user_id,
        sj.created_at AS job_created_at,
        dose_elem->'ratios' AS dose_ratios
    FROM simulation_jobs sj,
    jsonb_array_elements(sj.sim_result->'per_dose') AS dose_elem
    WHERE sj.status = 'done' AND sj.sim_result ? 'per_dose'
),
best_matches AS (
    SELECT DISTINCT ON (up.id)
        up.id AS pred_id,
        sd.job_id
    FROM prediction_logs up
    JOIN sim_doses sd 
      ON up.user_id = sd.user_id 
     AND up.created_at >= sd.job_created_at
     AND up.input = sd.dose_ratios
    WHERE up.job_id IS NULL AND up.source = 'digital_lab'
    ORDER BY up.id, sd.job_created_at DESC
)
UPDATE prediction_logs p
SET source = 'pipeline_dose',
    job_id = bm.job_id
FROM best_matches bm
WHERE p.id = bm.pred_id;

-- 7. Create indexes for efficient history list, run detail lookup, and foreign key traversal
CREATE INDEX IF NOT EXISTS idx_prediction_logs_user_created
    ON prediction_logs(user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_prediction_logs_job_id
    ON prediction_logs(job_id);

CREATE INDEX IF NOT EXISTS idx_prediction_logs_source
    ON prediction_logs(source);

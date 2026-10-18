CREATE TABLE IF NOT EXISTS simulation_jobs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid REFERENCES users(id) ON DELETE SET NULL,
  status text NOT NULL DEFAULT 'queued'
    CHECK (status IN ('queued','simulating','predicting','done','failed')),
  request jsonb NOT NULL,
  n_samples integer NOT NULL DEFAULT 1,
  sim_params jsonb,
  ratios jsonb,
  sim_result jsonb,
  prediction_id uuid REFERENCES prediction_logs(id) ON DELETE SET NULL,
  prediction jsonb,
  explanation jsonb,
  error text,
  sim_latency_ms integer,
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_simulation_jobs_user_created ON simulation_jobs(user_id, created_at DESC);

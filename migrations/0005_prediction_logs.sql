CREATE TABLE prediction_logs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid REFERENCES users(id) ON DELETE SET NULL,
    input jsonb NOT NULL,
    predicted_tier text,
    probabilities jsonb,
    latency_ms integer,
    created_at timestamptz NOT NULL DEFAULT now()
);

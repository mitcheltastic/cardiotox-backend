CREATE TABLE IF NOT EXISTS shap_logs (
    id             uuid primary key default gen_random_uuid(),
    user_id        uuid references users(id) on delete set null,
    prediction_id  uuid references prediction_logs(id) on delete set null,
    input          jsonb not null,
    predicted_class text,
    base_value     double precision,
    contributions  jsonb,
    latency_ms     integer,
    created_at     timestamptz not null default now()
);

CREATE INDEX idx_shap_logs_user_id ON shap_logs(user_id);
CREATE INDEX idx_shap_logs_prediction_id ON shap_logs(prediction_id);

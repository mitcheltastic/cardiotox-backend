ALTER TABLE prediction_logs
  ADD COLUMN prediction_set jsonb,
  ADD COLUMN recommended_action text,
  ADD COLUMN is_ambiguous boolean,
  ADD COLUMN out_of_distribution boolean,
  ADD COLUMN alpha double precision,
  ADD COLUMN q_hat double precision;

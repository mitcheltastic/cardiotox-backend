-- Migration 0012: Trace Retention Safeguard
-- Adds traces_evicted_at timestamp and partial index to support surgical waveform eviction.
-- Eviction only strips 'trace', 'control_trace', 'traces_by_dose' from sim_result.
-- Does NOT evict any existing data.

ALTER TABLE simulation_jobs 
ADD COLUMN IF NOT EXISTS traces_evicted_at TIMESTAMPTZ NULL;

CREATE INDEX IF NOT EXISTS idx_simulation_jobs_traces_retention 
ON simulation_jobs (user_id, created_at DESC) 
WHERE traces_evicted_at IS NULL;

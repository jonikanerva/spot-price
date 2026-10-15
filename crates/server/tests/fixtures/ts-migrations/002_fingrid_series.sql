-- Public Fingrid grid series for the FI price forecast.
-- Wind forecast (245), wind actual (75), consumption forecast (165),
-- consumption actual (124). These are public grid data, not user data
-- (VISION.md -> Persistence and Privacy Posture). One row per observation,
-- keyed by (dataset_id, start_time); the fetch job upserts idempotently and
-- prunes rows outside the needed window to bound growth (~35 days).
CREATE TABLE IF NOT EXISTS fingrid_series (
  dataset_id INTEGER NOT NULL,
  start_time TIMESTAMPTZ NOT NULL,
  end_time TIMESTAMPTZ NOT NULL,
  value DOUBLE PRECISION NOT NULL,
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (dataset_id, start_time)
);

-- Supports the read query WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
-- and the prune query (delete by dataset_id + start_time threshold).
CREATE INDEX IF NOT EXISTS idx_fingrid_series_dataset_start
  ON fingrid_series (dataset_id, start_time);

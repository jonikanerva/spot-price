-- Per-issuance VINTAGES of the Fingrid FORECAST datasets only — wind forecast
-- (245) and consumption forecast (165). These are public grid data, not user
-- data (VISION.md -> Persistence and Privacy Posture).
--
-- SINGLE HOME for the forecast datasets: forecasts live ONLY here, NOT in
-- `fingrid_series`. The two tables partition the four Fingrid datasets by how
-- they are revised:
--   * forecasts (245/165) are revised continuously toward the target, so we
--     keep ALL issuances here (append-only) and the live read takes the LATEST
--     issuance per target;
--   * actuals (75/124) are not meaningfully revised, so they stay upsert-latest
--     (one near-actual row per quarter) in `fingrid_series`.
-- The dual-write that briefly mirrored forecasts into `fingrid_series` was
-- rejected by the product owner as legacy-driven redundancy: keep-all vs
-- keep-latest gives each dataset class exactly one home.
--
-- !!! APPEND-ONLY PER ISSUANCE — NOT upsert-latest like `fingrid_series` !!!
-- storeFingridForecastVintages inserts with ON CONFLICT DO NOTHING (it does NOT
-- DO UPDATE), mirroring `weather_series`. Keeping every issuance is what makes
-- a later vintage-correct backtest (issue #80) and calibrated fit (#81)
-- leakage-free: they must train/evaluate on the forecast that was actually
-- available before the target, never on a hindsight-overwritten value. Never
-- "align" this table to a latest-only upsert — collapsing to one row per target
-- on STORE would destroy that property. (The LIVE route reads latest-per-target
-- at QUERY time via DISTINCT ON, which preserves all stored issuances.)
--
-- `issued_at` is an HOUR-TRUNCATED FETCH-TIME PROXY for issuance, not a
-- Fingrid-provided stamp: the hourly job records its own run instant truncated
-- to the hour, so the recorded issuance is within +/-1h of true issuance jitter.
-- That is the right granularity for the lead-time ladder the backtest reasons
-- about; do not treat it as exact. The PK below makes a re-run of the same
-- issuance hour idempotent without overwriting prior issuances.
CREATE TABLE IF NOT EXISTS fingrid_forecast_vintages (
  dataset_id INTEGER NOT NULL,
  issued_at TIMESTAMPTZ NOT NULL,
  start_time TIMESTAMPTZ NOT NULL,
  end_time TIMESTAMPTZ NOT NULL,
  value DOUBLE PRECISION NOT NULL,
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (dataset_id, issued_at, start_time)
);

-- Backs the LIVE latest-per-target read:
--   SELECT DISTINCT ON (start_time) ...
--   WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
--   ORDER BY start_time, issued_at DESC
-- The trailing `issued_at DESC` lets the planner satisfy the DISTINCT ON via an
-- index skip with no Sort / HashAggregate node (see the EXPLAIN in PR #82).
CREATE INDEX IF NOT EXISTS idx_fingrid_vintages_target_issued
  ON fingrid_forecast_vintages (dataset_id, start_time, issued_at DESC);
-- Backs the retention prune DELETE WHERE issued_at < $1.
CREATE INDEX IF NOT EXISTS idx_fingrid_vintages_issued
  ON fingrid_forecast_vintages (issued_at);

-- Per-issuance VINTAGES of the Fingrid FORECAST datasets only — wind forecast
-- (245) and consumption forecast (165). These are public grid data, not user
-- data (VISION.md -> Persistence and Privacy Posture).
--
-- This is a SEPARATE table; it does NOT replace `fingrid_series`. The ACTUAL
-- datasets (75 wind, 124 consumption) are NOT stored here — they stay
-- upsert-latest in `fingrid_series` (one near-actual row per quarter), which the
-- live forecast read path keeps reading unchanged.
--
-- !!! APPEND-ONLY PER ISSUANCE — NOT upsert-latest like `fingrid_series` !!!
-- storeFingridForecastVintages inserts with ON CONFLICT DO NOTHING (it does NOT
-- DO UPDATE). This is deliberate and load-bearing, mirroring `weather_series`:
-- the Fingrid forecast datasets are revised continuously as the target time
-- approaches, so the upsert-latest `fingrid_series` keeps only the final
-- (near-actual) revision and discards WHAT THE FORECAST SAID AT EACH ISSUE
-- TIME. A later vintage-correct backtest (issue #80) and calibrated fit (#81)
-- must train/evaluate on the forecast that was actually available before the
-- target, never on a hindsight-overwritten value. Never "align" this table to
-- the Fingrid upsert — collapsing to the latest issuance would silently destroy
-- the leakage-free property and make the backtest optimistic.
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

-- Supports the range read WHERE dataset_id = $1 AND start_time >= $2
-- AND start_time < $3 (issue #80 will add an as-of issuance selection on top)
-- and the prune DELETE WHERE issued_at < $1.
CREATE INDEX IF NOT EXISTS idx_fingrid_vintages_dataset_start
  ON fingrid_forecast_vintages (dataset_id, start_time);
CREATE INDEX IF NOT EXISTS idx_fingrid_vintages_issued
  ON fingrid_forecast_vintages (issued_at);

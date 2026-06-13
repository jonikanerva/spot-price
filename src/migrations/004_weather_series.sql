-- Public OpenWeatherMap weather forecasts for the FI price forecast (Phase 1:
-- forward-only collection; no change to any price/forecast response). One row
-- per (point, issuance hour, target quarter/hour) carrying the forecast as it
-- was ISSUED at `issued_at`. These are public weather data, not user data
-- (VISION.md -> Persistence and Privacy Posture).
--
-- !!! APPEND-ONLY PER ISSUANCE — NOT upsert-latest like `fingrid_series` !!!
-- storeWeatherRecords inserts with ON CONFLICT DO NOTHING (it does NOT
-- DO UPDATE). This is deliberate and load-bearing: each hourly run stores a
-- NEW `issued_at`, so the same `target_time` accumulates multiple rows — one
-- per issuance — preserving WHAT THE FORECAST SAID AT EACH ISSUE TIME. A later
-- weather-feature backtest (Phase 3) must train/evaluate on the forecast that
-- was actually available before the target, never on a hindsight-overwritten
-- "best" value. Never "align" this table to the Fingrid upsert — collapsing to
-- the latest issuance would silently destroy the leakage-free property and make
-- the backtest optimistic. The PK below is what makes a re-run of the same
-- issuance idempotent without overwriting prior issuances.
CREATE TABLE IF NOT EXISTS weather_series (
  point_id TEXT NOT NULL,
  issued_at TIMESTAMPTZ NOT NULL,
  target_time TIMESTAMPTZ NOT NULL,
  temp DOUBLE PRECISION NOT NULL,
  clouds DOUBLE PRECISION NOT NULL,
  uvi DOUBLE PRECISION NOT NULL,
  wind_speed DOUBLE PRECISION NOT NULL,
  wind_deg DOUBLE PRECISION NOT NULL,
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (point_id, issued_at, target_time)
);

-- Supports the backtest read query
-- WHERE point_id = $1 AND target_time >= $2 AND target_time < $3.
CREATE INDEX IF NOT EXISTS idx_weather_series_point_target
  ON weather_series (point_id, target_time);

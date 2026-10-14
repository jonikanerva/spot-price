-- Public OpenWeatherMap weather forecasts for the FI price forecast (Phase 1:
-- forward-only collection; no change to any price/forecast response). One row
-- per (point, issuance hour, target quarter/hour) carrying the forecast as it
-- was ISSUED at `issued_at`. These are public weather data, not user data
-- (VISION.md -> Persistence and Privacy Posture).
--
-- !!! APPEND-ONLY PER ISSUANCE — NOT upsert-latest like `fingrid_series` !!!
-- storeWeatherRecords inserts with ON CONFLICT DO NOTHING. Never "align" this
-- table to the Fingrid upsert — see STACK.md section 5.
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

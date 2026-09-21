-- Public OpenWeatherMap DAILY weather forecasts for the FI price forecast
-- (collection only — no change to any price/forecast response). One
-- row per (point, issuance hour, target day) carrying the daily block as it was
-- ISSUED at `issued_at`. These are public weather data, not user data
-- (VISION.md -> Persistence and Privacy Posture).
--
-- !!! APPEND-ONLY PER ISSUANCE — NOT upsert-latest like `fingrid_actuals` !!!
-- storeWeatherDailyRecords inserts with ON CONFLICT DO NOTHING. Never "align"
-- this table to an upsert — see STACK.md section 5.
--
-- `target_date` is the UTC CALENDAR DATE of `target_dt`, derived as the first
-- ten characters of the UTC ISO instant. No local-time arithmetic is used:
-- STACK.md section 7 forbids it below the response boundary, and OWM's
-- `daily[].dt` is LOCAL NOON at the point, so defining the day locally would
-- drag a timezone (and the DST fall-back) into storage.
-- CONSTRAINT of the UTC derivation: it equals the point's local calendar day
-- for any point between UTC-11 and UTC+11. Every point in `WEATHER_POINTS` is
-- Finnish, far inside that range. A point outside it would need an explicit
-- decision, not a silent drift — the constraint is pinned by a unit test.
--
-- `target_dt` keeps the raw upstream instant as PROVENANCE, so the date
-- derivation stays recomputable from stored data without re-collecting. Weather
-- collection is forward-only and irreversible: a value not collected today can
-- never be recovered for today.
--
-- `sunrise` / `sunset` are NULLABLE: One Call omits them at polar latitudes
-- during midnight sun and polar night. Helsinki (60N) and Vaasa (63N) never hit
-- that, but `WEATHER_POINTS` is documented as extensible.
--
-- All six `temp` sub-fields are collected although a first feature would use
-- three. Collection is forward-only and irreversible: a value not collected
-- today can never be recovered for today. A first feature layer uses `day`,
-- `min` and `max` only; `night`, `eve` and `morn` stay unused until a
-- measurement justifies them. Retention: see STACK.md section 5.
CREATE TABLE IF NOT EXISTS weather_daily_forecasts (
  point_id TEXT NOT NULL,
  issued_at TIMESTAMPTZ NOT NULL,
  target_date DATE NOT NULL,
  target_dt TIMESTAMPTZ NOT NULL,
  temp_morn DOUBLE PRECISION NOT NULL,
  temp_day DOUBLE PRECISION NOT NULL,
  temp_eve DOUBLE PRECISION NOT NULL,
  temp_night DOUBLE PRECISION NOT NULL,
  temp_min DOUBLE PRECISION NOT NULL,
  temp_max DOUBLE PRECISION NOT NULL,
  clouds DOUBLE PRECISION NOT NULL,
  uvi DOUBLE PRECISION NOT NULL,
  sunrise TIMESTAMPTZ,
  sunset TIMESTAMPTZ,
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (point_id, issued_at, target_date)
);

-- NO further indexes on purpose. The only queries today are the PK-keyed insert
-- and the retention prune (DELETE WHERE issued_at < $1), which at ~154k rows in
-- steady state is a cheap scan on a small table. The read path is designed in
-- a later change; adding a speculative index now would cost write amplification on
-- every hourly run for a query that does not exist yet.

-- Normalize the Fingrid/weather table names to content-based names, matching
-- how each table is maintained:
--   * fingrid_series   -> fingrid_actuals     (actuals 75/124, kept-latest)
--   * weather_series   -> weather_forecasts   (forecasts, kept-all per issuance)
--
-- These tables are already DEPLOYED, so they are renamed in place rather than
-- recreated. ALTER ... IF EXISTS keeps this idempotent/robust across
-- environments (e.g. a fresh DB where 002/004 created the tables under the old
-- names, vs. any environment already part-migrated). No foreign keys reference
-- these tables, so a plain RENAME is safe; the Better Auth tables are untouched.

-- fingrid_series -> fingrid_actuals (+ its explicit and primary-key indexes).
ALTER TABLE IF EXISTS fingrid_series RENAME TO fingrid_actuals;
ALTER INDEX IF EXISTS idx_fingrid_series_dataset_start
  RENAME TO idx_fingrid_actuals_dataset_start;
ALTER INDEX IF EXISTS fingrid_series_pkey RENAME TO fingrid_actuals_pkey;

-- weather_series -> weather_forecasts (+ its explicit and primary-key indexes).
ALTER TABLE IF EXISTS weather_series RENAME TO weather_forecasts;
ALTER INDEX IF EXISTS idx_weather_series_point_target
  RENAME TO idx_weather_forecasts_point_target;
ALTER INDEX IF EXISTS weather_series_pkey RENAME TO weather_forecasts_pkey;

-- Purge now-orphaned forecast rows from the actuals table. Before single-home,
-- the forecast datasets (245 wind, 165 consumption) were also upserted into
-- fingrid_series; forecasts now live only in `fingrid_forecasts` and nothing
-- reads 245/165 from the actuals table. Deleting them makes the table
-- truthfully actuals-only and reclaims storage. Safe: the live route reads
-- forecasts from `fingrid_forecasts`, so this cannot change any response.
DELETE FROM fingrid_actuals WHERE dataset_id IN (245, 165);

-- Public day-ahead prices for the browser journeys of `mise run e2e`.
--
-- The rows cover every Nord Pool area in 15-minute slots from the UTC day
-- before today to the end of the UTC day after tomorrow. The price fetch job
-- skips a UTC date when every area has its rows, so the server makes no Nord
-- Pool call at start-up or on a scheduled tick, also when a run crosses UTC
-- midnight. The public chart of the landing page also reads these rows.
INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
SELECT
  slot,
  slot + INTERVAL '15 minutes',
  -- A fixed daily shape: 20.0 EUR/MWh at 00:00 UTC, 77.5 EUR/MWh at 23:00 UTC.
  20.0 + 2.5 * EXTRACT(HOUR FROM slot AT TIME ZONE 'UTC'),
  area
FROM
  unnest(ARRAY[
    'FI', 'SE1', 'SE2', 'SE3', 'SE4', 'NO1', 'NO2', 'NO3', 'NO4', 'NO5',
    'DK1', 'DK2', 'EE', 'LT', 'LV', 'AT', 'BE', 'FR', 'GER', 'NL', 'PL'
  ]) AS area,
  generate_series(
    date_trunc('day', now(), 'UTC') - INTERVAL '1 day',
    date_trunc('day', now(), 'UTC') + INTERVAL '3 days' - INTERVAL '15 minutes',
    INTERVAL '15 minutes'
  ) AS slot
ON CONFLICT (delivery_start, area) DO NOTHING;

-- Per-issuance vintages of the Fingrid FORECAST datasets only — wind forecast
-- (245) and consumption forecast (165). These are public grid data, not user
-- data (VISION.md -> Persistence and Privacy Posture).
--
-- SINGLE HOME for the forecast datasets: forecasts live ONLY here, NOT in
-- `fingrid_actuals`. The two tables partition the four Fingrid datasets by how
-- they are revised (content-based names):
--   * forecasts (245/165) are revised continuously toward the target, so we
--     keep ALL issuances here (append-only) and the live read takes the LATEST
--     issuance per target;
--   * actuals (75/124) are not meaningfully revised, so they stay upsert-latest
--     (one near-actual row per quarter) in `fingrid_actuals`.
-- The dual-write that briefly mirrored forecasts into the actuals table was
-- rejected by the product owner as legacy-driven redundancy: keep-all vs
-- keep-latest gives each dataset class exactly one home.
--
-- !!! APPEND-ONLY PER ISSUANCE — NOT upsert-latest like `fingrid_actuals` !!!
-- storeFingridForecastVintages inserts with ON CONFLICT DO NOTHING (it does NOT
-- DO UPDATE), mirroring `weather_forecasts`. Keeping every issuance is what
-- makes a later vintage-correct backtest (issue #80) and calibrated fit (#81)
-- leakage-free: they must train/evaluate on the forecast that was actually
-- available before the target, never on a hindsight-overwritten value. Never
-- "align" this table to a latest-only upsert — collapsing to one row per target
-- on STORE would destroy that property. (The LIVE route reads latest-per-target
-- at QUERY time via a LATERAL skip-scan — see the index comment below — which
-- preserves all stored issuances.)
--
-- `issued_at` is an HOUR-TRUNCATED FETCH-TIME PROXY for issuance, not a
-- Fingrid-provided stamp: the hourly job records its own run instant truncated
-- to the hour, so the recorded issuance is within +/-1h of true issuance jitter.
-- That is the right granularity for the lead-time ladder the backtest reasons
-- about; do not treat it as exact. The PK below makes a re-run of the same
-- issuance hour idempotent without overwriting prior issuances.
CREATE TABLE IF NOT EXISTS fingrid_forecasts (
  dataset_id INTEGER NOT NULL,
  issued_at TIMESTAMPTZ NOT NULL,
  start_time TIMESTAMPTZ NOT NULL,
  end_time TIMESTAMPTZ NOT NULL,
  value DOUBLE PRECISION NOT NULL,
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (dataset_id, issued_at, start_time)
);

-- Backs the LIVE latest-per-target read (`getFingridForecastVintagesLatest`),
-- which is a LATERAL skip-scan, NOT `DISTINCT ON`. This index serves BOTH legs:
--   1. the inner `SELECT DISTINCT start_time WHERE dataset_id=$1 AND
--      start_time >= $2 AND start_time < $3` (the leading columns make this an
--      index-only scan over the in-range targets);
--   2. the per-target LATERAL `... WHERE dataset_id=$1 AND start_time=target
--      ORDER BY issued_at DESC LIMIT 1` — the trailing `issued_at DESC` makes
--      the newest issuance a single index seek (no sort) per target.
-- `DISTINCT ON (start_time) ... ORDER BY start_time, issued_at DESC` was tried
-- and REJECTED: Postgres has no loose/skip index scan for DISTINCT ON, so at
-- 180-day depth (~72 issuances/target for 245) it read every in-range row and
-- sorted them to disk (external-merge Sort, ~116 ms cold > the STACK §4 100 ms
-- p99). The LATERAL stays ~48 ms warm / ~67 ms cold (EXPLAIN in PR #82). Do NOT
-- "simplify" the live read back to DISTINCT ON — it reintroduces that
-- regression. (#80 adds an as-of bound `AND issued_at <= $asOf` inside leg 2;
-- it composes cleanly and keeps using this index.)
CREATE INDEX IF NOT EXISTS idx_fingrid_forecasts_target_issued
  ON fingrid_forecasts (dataset_id, start_time, issued_at DESC);
-- Backs the retention prune DELETE WHERE issued_at < $1.
CREATE INDEX IF NOT EXISTS idx_fingrid_forecasts_issued
  ON fingrid_forecasts (issued_at);

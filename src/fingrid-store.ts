import type { Pool } from "pg";
import {
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_FORECAST,
} from "./fingrid.js";
import type { FingridRecord, ForecastVintageRecord } from "./types.js";

/**
 * Persistence for the public Fingrid grid series (read off the request path
 * only; the forecast route reads from this table and never calls Fingrid
 * synchronously). Mirrors `price-store.ts`: idempotent upsert keyed by
 * (dataset_id, start_time), range reads, and a prune to bound table growth.
 *
 * This file also owns the SEPARATE `fingrid_forecasts` table (issue
 * #78), the SINGLE HOME for the FORECAST datasets (245/165): every issuance is
 * archived append-only, and the live route reads the latest issuance per target
 * via `getFingridForecastVintagesLatest`. Forecasts are NOT written to
 * `fingrid_actuals` (only actuals 75/124 are). The two stores never share a
 * transaction — the authoritative actuals upsert below must never be aborted by
 * a vintage-write failure.
 */

/** Upsert Fingrid records (idempotent via ON CONFLICT). Returns rows written. */
export const storeFingridRecords = async (
  pool: Pool,
  records: readonly FingridRecord[],
): Promise<number> => {
  if (records.length === 0) {
    return 0;
  }

  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    for (const r of records) {
      await client.query(
        `INSERT INTO fingrid_actuals (dataset_id, start_time, end_time, value)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (dataset_id, start_time)
         DO UPDATE SET end_time = EXCLUDED.end_time,
                       value = EXCLUDED.value,
                       fetched_at = NOW()`,
        [r.datasetId, r.startTime, r.endTime, r.value],
      );
    }
    await client.query("COMMIT");
  } catch (err) {
    await client.query("ROLLBACK");
    throw err;
  } finally {
    client.release();
  }

  return records.length;
};

interface FingridRow {
  dataset_id: number;
  start_time: string;
  end_time: string;
  value: number;
}

const rowToRecord = (r: FingridRow): FingridRecord => ({
  datasetId: r.dataset_id,
  startTime: r.start_time,
  endTime: r.end_time,
  value: r.value,
});

/**
 * Read records for a dataset within a UTC time range.
 * Returns entries where start_time >= startUtc AND start_time < endUtc.
 */
export const getFingridRecordsByRange = async (
  pool: Pool,
  datasetId: number,
  startUtc: string,
  endUtc: string,
): Promise<readonly FingridRecord[]> => {
  const { rows } = await pool.query<FingridRow>(
    `SELECT dataset_id, start_time, end_time, value
     FROM fingrid_actuals
     WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
     ORDER BY start_time`,
    [datasetId, startUtc, endUtc],
  );
  return rows.map(rowToRecord);
};

/**
 * Delete rows older than `beforeUtc` to bound table growth. Called by the
 * fetch job after each successful fetch so the table holds roughly the needed
 * window (~35 days). Returns the number of rows pruned.
 */
export const pruneFingridRecordsBefore = async (
  pool: Pool,
  beforeUtc: string,
): Promise<number> => {
  const result = await pool.query(
    `DELETE FROM fingrid_actuals WHERE start_time < $1`,
    [beforeUtc],
  );
  return result.rowCount ?? 0;
};

// ---------------------------------------------------------------------------
// Per-issuance vintages of the FORECAST datasets (issue #78)
//
// Mirrors `weather-store.ts`: APPEND-ONLY per issuance
// (`ON CONFLICT DO NOTHING`), prune by ISSUANCE, plain range read. The actual
// datasets (75/124) are deliberately never written here — see the guard in
// `storeFingridForecastVintages` — they stay upsert-latest in `fingrid_actuals`.
// ---------------------------------------------------------------------------

/** Forecast datasets whose vintages we archive; actuals are excluded. */
const VINTAGE_DATASETS: ReadonlySet<number> = new Set([
  DATASET_WIND_FORECAST,
  DATASET_CONSUMPTION_FORECAST,
]);

/**
 * Archive forecast-dataset records under one issuance, append-only per issuance
 * (idempotent via `ON CONFLICT (dataset_id, issued_at, start_time) DO NOTHING`).
 *
 * Records are filtered to the forecast datasets (245/165) INTERNALLY against
 * `VINTAGE_DATASETS`, so an actual (75/124) handed in by the caller can never be
 * archived here — the vintage table holds forecast vintages only. `issuedAt` is
 * the hour-truncated fetch-time proxy supplied by the job. Returns the number of
 * rows actually inserted (re-running the same issuance inserts none).
 */
export const storeFingridForecastVintages = async (
  pool: Pool,
  issuedAt: string,
  records: readonly FingridRecord[],
): Promise<number> => {
  const forecastRecords = records.filter((r) =>
    VINTAGE_DATASETS.has(r.datasetId),
  );
  if (forecastRecords.length === 0) {
    return 0;
  }

  let inserted = 0;
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    for (const r of forecastRecords) {
      const result = await client.query(
        `INSERT INTO fingrid_forecasts
           (dataset_id, issued_at, start_time, end_time, value)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (dataset_id, issued_at, start_time) DO NOTHING`,
        [r.datasetId, issuedAt, r.startTime, r.endTime, r.value],
      );
      inserted += result.rowCount ?? 0;
    }
    await client.query("COMMIT");
  } catch (err) {
    await client.query("ROLLBACK");
    throw err;
  } finally {
    client.release();
  }

  return inserted;
};

/**
 * LIVE read for the forecast route: the LATEST issuance per target quarter in
 * [startUtc, endUtc). Returns exactly ONE row per `start_time` — the most
 * recently issued vintage — which is the production-correct value (the freshest
 * forecast available now).
 *
 * Returning exactly one row per quarter is load-bearing, not just tidy: the
 * pure pipeline buckets records by quarter with `bucketRecords`
 * (`forecast.ts`), which takes the MEAN when several rows share a quarter.
 * Multiple issuances per target would silently average a stale (+44h-old) and a
 * fresh (+1h-old) forecast into a wrong value with no error — the per-target
 * `LIMIT 1` is what prevents that.
 *
 * Shape: a LATERAL skip-scan, NOT `DISTINCT ON`. Postgres has no loose/skip
 * index scan for `DISTINCT ON`, so over a deep table (each target carries up to
 * one issuance per forecast-horizon hour — ~72 for dataset 245) the planner
 * reads EVERY in-range issuance and sorts them (an external-merge Sort that
 * spilled to disk and ran ~66 ms warm / ~116 ms cold at 180-day depth — over
 * the STACK §4 100 ms p99 budget). Instead: the inner `SELECT DISTINCT
 * start_time` is satisfied by an index-only scan on
 * `(dataset_id, start_time, issued_at DESC)`, then for each target the LATERAL
 * does an index seek + `LIMIT 1` to grab the newest issuance — turning the deep
 * scan into ~one index probe per target. Measured ~37 ms at 180-day depth for
 * dataset 245 (see PR #82 EXPLAIN).
 *
 * NOTE: this takes the newest issuance unconditionally; the as-of selection
 * (`AND f.issued_at <= $asOf` inside the LATERAL, for the vintage-correct
 * backtest) is deferred to #80 — it composes cleanly into this shape.
 */
export const getFingridForecastVintagesLatest = async (
  pool: Pool,
  datasetId: number,
  startUtc: string,
  endUtc: string,
): Promise<readonly FingridRecord[]> => {
  const { rows } = await pool.query<FingridRow>(
    `SELECT v.dataset_id, v.start_time, v.end_time, v.value
     FROM (
       SELECT DISTINCT start_time
       FROM fingrid_forecasts
       WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
     ) AS targets
     CROSS JOIN LATERAL (
       SELECT dataset_id, start_time, end_time, value
       FROM fingrid_forecasts f
       WHERE f.dataset_id = $1 AND f.start_time = targets.start_time
       -- #80 adds the as-of bound here: AND f.issued_at <= $asOf
       ORDER BY f.issued_at DESC
       LIMIT 1
     ) AS v
     ORDER BY v.start_time`,
    [datasetId, startUtc, endUtc],
  );
  return rows.map(rowToRecord);
};

interface ForecastVintageRow {
  dataset_id: number;
  issued_at: string;
  start_time: string;
  end_time: string;
  value: number;
}

/**
 * OFFLINE read: EVERY archived issuance per target in [startUtc, endUtc) for one
 * forecast dataset, ordered by (start_time, issued_at) — the full lead-time
 * ladder. The revision study (#79) and the vintage-correct backtest (#80) need
 * all issuances, unlike `getFingridForecastVintagesLatest`, which collapses to
 * the latest per target for the live route. The server never calls this; it
 * lives here (next to the latest-per-target read) so #79 and #80 share ONE query
 * and row mapping rather than duplicating the SQL in `tools/`. Backed by
 * `idx_fingrid_forecasts_target_issued` (leading `dataset_id, start_time`).
 */
export const getFingridForecastVintagesAll = async (
  pool: Pool,
  datasetId: number,
  startUtc: string,
  endUtc: string,
): Promise<readonly ForecastVintageRecord[]> => {
  const { rows } = await pool.query<ForecastVintageRow>(
    `SELECT dataset_id, issued_at, start_time, end_time, value
     FROM fingrid_forecasts
     WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
     ORDER BY start_time, issued_at`,
    [datasetId, startUtc, endUtc],
  );
  return rows.map((r) => ({
    datasetId: r.dataset_id,
    issuedAt: r.issued_at,
    startTime: r.start_time,
    endTime: r.end_time,
    value: r.value,
  }));
};

/**
 * Delete vintages whose ISSUANCE is older than `beforeUtc` to bound table
 * growth. Pruning by `issued_at` (not `start_time`, like `fingrid_actuals`) keeps
 * the retention window aligned to when a forecast was made — the unit the
 * backtest reasons about, mirroring `weather-store.ts`. Returns the row count.
 */
export const pruneFingridForecastVintagesBefore = async (
  pool: Pool,
  beforeUtc: string,
): Promise<number> => {
  const result = await pool.query(
    `DELETE FROM fingrid_forecasts WHERE issued_at < $1`,
    [beforeUtc],
  );
  return result.rowCount ?? 0;
};

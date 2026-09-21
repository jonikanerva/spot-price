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
 * This file also owns the SEPARATE `fingrid_forecasts` table, the SINGLE HOME
 * for the FORECAST datasets (245/165): every issuance is
 * archived append-only, and the live route reads the latest issuance per target
 * via `getFingridForecastVintagesLatest`. Forecasts are NOT written to
 * `fingrid_actuals` (only actuals 75/124 are). The two stores never share a
 * transaction — the authoritative actuals upsert below must never be aborted by
 * a vintage-write failure.
 */

/**
 * Upsert Fingrid ACTUAL records (idempotent via ON CONFLICT).
 *
 * The `DO UPDATE` carries a change guard: a row is rewritten only when its
 * `(end_time, value)` pair differs from the incoming one. `IS DISTINCT FROM`
 * treats NULL as a value, so a NULL pair is never read as a change. The guard
 * addresses the stored row by TABLE NAME, not `EXCLUDED`; the syntax requires
 * that. `fetched_at` therefore means LAST CHANGED — see `STACK.md §5`.
 */
/**
 * Returns the number of records HANDED IN, not the number of rows written. The
 * change guard must not leak into this count: `stored` means "this many
 * observations were accepted". `fetch-job.ts` derives `tomorrowAvailable` from
 * the sibling price store's count, so a 0 for unchanged data would teach the
 * scheduler that prices are missing and add upstream calls.
 */
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
                       fetched_at = NOW()
         WHERE (fingrid_actuals.end_time, fingrid_actuals.value)
               IS DISTINCT FROM (EXCLUDED.end_time, EXCLUDED.value)`,
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
// Per-issuance vintages of the FORECAST datasets
// ---------------------------------------------------------------------------

/** Forecast datasets whose vintages we archive; actuals are excluded. */
const VINTAGE_DATASETS: ReadonlySet<number> = new Set([
  DATASET_WIND_FORECAST,
  DATASET_CONSUMPTION_FORECAST,
]);

const HOUR_MS = 60 * 60 * 1000;

/**
 * How far BEFORE the issuance a target may lie and still be archived. This is
 * an OUTAGE-TOLERANCE window, not a "keep one post-delivery
 * reference" knob.
 *
 * It lives here, beside `VINTAGE_DATASETS`, because the store owns the archive
 * write policy and both guards belong together. `forecast-job.ts` already
 * imports this module, so the reverse placement would create an import cycle.
 */
export const VINTAGE_BACKFILL_HOURS = 6;

/**
 * Archive forecast-dataset records under one issuance, append-only per issuance
 * (idempotent via `ON CONFLICT (dataset_id, issued_at, start_time) DO NOTHING`).
 *
 * TWO guards run in the SAME filter pass, both INTERNAL, so no caller can widen
 * either one:
 *
 *  1. DATASET — only the forecast datasets (245/165) in `VINTAGE_DATASETS` pass.
 *     An actual (75/124) handed in by the caller can never be archived here; the
 *     vintage table holds forecast vintages only.
 *  2. TARGET WINDOW — only targets with
 *     `start_time >= issuedAt − VINTAGE_BACKFILL_HOURS` pass.
 *
 * The window threshold is computed ONCE, outside the filter, and compared on
 * parsed epoch milliseconds. `startTime` is an unnormalised upstream string
 * (`fingrid.ts` passes it through as `z.string()`) while `issuedAt` is a
 * canonical `toISOString()`, so a lexicographic comparison would be wrong. The
 * bound is INCLUSIVE: `issuedAt` is hour-truncated, so the quarters of the
 * current hour are still a fresh forecast. A `start_time` that does not parse
 * yields `NaN`, and `NaN >= x` is false, so such a record drops here BY DESIGN —
 * the `timestamptz` column would reject it anyway.
 *
 * `issuedAt` is the hour-truncated fetch-time proxy supplied by the job, within
 * ±1 h of true issuance. Do not treat it as exact.
 *
 * Returns the number of rows actually inserted (re-running the same issuance
 * inserts none).
 */
export const storeFingridForecastVintages = async (
  pool: Pool,
  issuedAt: string,
  records: readonly FingridRecord[],
): Promise<number> => {
  const oldestArchivedMs =
    Date.parse(issuedAt) - VINTAGE_BACKFILL_HOURS * HOUR_MS;
  const forecastRecords = records.filter(
    (r) =>
      VINTAGE_DATASETS.has(r.datasetId) &&
      Date.parse(r.startTime) >= oldestArchivedMs,
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
 * [startUtc, endUtc). Returns exactly ONE row per `start_time`.
 *
 * One row per quarter is load-bearing. `bucketRecords` (`forecast.ts`) takes the
 * MEAN when several rows share a quarter, so a second issuance would silently
 * average a stale and a fresh forecast into a wrong value with no error.
 *
 * Do not replace the LATERAL skip-scan with `DISTINCT ON` — see `STACK.md §5`.
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
 * ladder. The offline studies need all issuances, unlike
 * `getFingridForecastVintagesLatest`, which collapses to the latest per target
 * for the live route. The server never calls this. It lives here, next to the
 * latest-per-target read, so every caller shares ONE query and row mapping
 * rather than duplicating the SQL in `tools/`. Backed by
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

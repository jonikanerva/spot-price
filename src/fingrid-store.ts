import type { Pool } from "pg";
import {
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_FORECAST,
} from "./fingrid.js";
import type { FingridRecord } from "./types.js";

/**
 * Persistence for the public Fingrid grid series (read off the request path
 * only; the forecast route reads from this table and never calls Fingrid
 * synchronously). Mirrors `price-store.ts`: idempotent upsert keyed by
 * (dataset_id, start_time), range reads, and a prune to bound table growth.
 *
 * This file also owns the SEPARATE `fingrid_forecast_vintages` table (issue
 * #78): per-issuance archival of the FORECAST datasets only (245/165), so the
 * leakage-free history a later vintage-correct backtest (#80) needs is kept.
 * The two stores never share a transaction — the authoritative upsert below
 * must never be aborted by a vintage-write failure.
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
        `INSERT INTO fingrid_series (dataset_id, start_time, end_time, value)
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
     FROM fingrid_series
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
    `DELETE FROM fingrid_series WHERE start_time < $1`,
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
// `storeFingridForecastVintages` — they stay upsert-latest in `fingrid_series`.
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
        `INSERT INTO fingrid_forecast_vintages
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
 * Plain range read over the vintage table: every archived issuance whose target
 * quarter falls within [startUtc, endUtc), ordered so that for a given target
 * the issuances arrive oldest-first. Issue #80 will add the as-of issuance
 * selection (`DISTINCT ON (start_time) ... issued_at <= asOf`) on top of this.
 */
export const getFingridForecastVintagesByRange = async (
  pool: Pool,
  datasetId: number,
  startUtc: string,
  endUtc: string,
): Promise<readonly FingridRecord[]> => {
  const { rows } = await pool.query<FingridRow>(
    `SELECT dataset_id, start_time, end_time, value
     FROM fingrid_forecast_vintages
     WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
     ORDER BY start_time, issued_at`,
    [datasetId, startUtc, endUtc],
  );
  return rows.map(rowToRecord);
};

/**
 * Delete vintages whose ISSUANCE is older than `beforeUtc` to bound table
 * growth. Pruning by `issued_at` (not `start_time`, like `fingrid_series`) keeps
 * the retention window aligned to when a forecast was made — the unit the
 * backtest reasons about, mirroring `weather-store.ts`. Returns the row count.
 */
export const pruneFingridForecastVintagesBefore = async (
  pool: Pool,
  beforeUtc: string,
): Promise<number> => {
  const result = await pool.query(
    `DELETE FROM fingrid_forecast_vintages WHERE issued_at < $1`,
    [beforeUtc],
  );
  return result.rowCount ?? 0;
};

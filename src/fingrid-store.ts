import type { Pool } from "pg";
import type { FingridRecord } from "./types.js";

/**
 * Persistence for the public Fingrid grid series (read off the request path
 * only; the forecast route reads from this table and never calls Fingrid
 * synchronously). Mirrors `price-store.ts`: idempotent upsert keyed by
 * (dataset_id, start_time), range reads, and a prune to bound table growth.
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

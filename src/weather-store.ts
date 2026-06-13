import type { Pool } from "pg";
import type { WeatherRecord } from "./types.js";

/**
 * Persistence for the public OpenWeatherMap weather forecasts (read off the
 * request path only; nothing on the price/forecast hot path touches this table
 * in Phase 1). Mirrors `fingrid-store.ts` in shape — keyed insert, range read,
 * prune — with ONE deliberate difference: the insert is APPEND-ONLY per
 * issuance (`ON CONFLICT DO NOTHING`), NOT the upsert-latest that
 * `fingrid-store.ts` uses.
 *
 * Why append-only: each hourly run carries a fresh `issued_at`, so the same
 * `target_time` accumulates one row per issuance, preserving what the forecast
 * SAID at each issue time. A later weather-feature backtest (issue #73 Phase 3)
 * must evaluate on the pre-target forecast, never a hindsight-overwritten
 * value. Overwriting to the latest issuance would silently destroy that
 * leakage-free property — never convert this to a DO UPDATE.
 */

/**
 * Insert weather records, append-only per issuance (idempotent via
 * `ON CONFLICT (point_id, issued_at, target_time) DO NOTHING`). Returns the
 * number of rows actually inserted (re-running the same issuance inserts none).
 */
export const storeWeatherRecords = async (
  pool: Pool,
  records: readonly WeatherRecord[],
): Promise<number> => {
  if (records.length === 0) {
    return 0;
  }

  let inserted = 0;
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    for (const r of records) {
      const result = await client.query(
        `INSERT INTO weather_series
           (point_id, issued_at, target_time, temp, clouds, uvi, wind_speed, wind_deg)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (point_id, issued_at, target_time) DO NOTHING`,
        [
          r.pointId,
          r.issuedAt,
          r.targetTime,
          r.temp,
          r.clouds,
          r.uvi,
          r.windSpeed,
          r.windDeg,
        ],
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

interface WeatherRow {
  point_id: string;
  issued_at: string;
  target_time: string;
  temp: number;
  clouds: number;
  uvi: number;
  wind_speed: number;
  wind_deg: number;
}

const rowToRecord = (r: WeatherRow): WeatherRecord => ({
  pointId: r.point_id,
  issuedAt: r.issued_at,
  targetTime: r.target_time,
  temp: r.temp,
  clouds: r.clouds,
  uvi: r.uvi,
  windSpeed: r.wind_speed,
  windDeg: r.wind_deg,
});

/**
 * Read records for a point whose target falls within a UTC range, for the
 * future weather-feature backtest. Returns entries where
 * target_time >= startUtc AND target_time < endUtc, ordered so that for a given
 * target the issuances arrive oldest-first.
 */
export const getWeatherRecordsByRange = async (
  pool: Pool,
  pointId: string,
  startUtc: string,
  endUtc: string,
): Promise<readonly WeatherRecord[]> => {
  const { rows } = await pool.query<WeatherRow>(
    `SELECT point_id, issued_at, target_time, temp, clouds, uvi, wind_speed, wind_deg
     FROM weather_series
     WHERE point_id = $1 AND target_time >= $2 AND target_time < $3
     ORDER BY target_time, issued_at`,
    [pointId, startUtc, endUtc],
  );
  return rows.map(rowToRecord);
};

/**
 * Delete rows whose issuance is older than `beforeUtc` to bound table growth.
 * Pruning by `issued_at` (not `target_time`) keeps the retention window aligned
 * to when a forecast was made — the unit the backtest reasons about. Returns
 * the number of rows pruned.
 */
export const pruneWeatherRecordsBefore = async (
  pool: Pool,
  beforeUtc: string,
): Promise<number> => {
  const result = await pool.query(
    `DELETE FROM weather_series WHERE issued_at < $1`,
    [beforeUtc],
  );
  return result.rowCount ?? 0;
};

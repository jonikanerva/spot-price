import type { Pool } from "pg";
import { fetchWeather, WEATHER_POINTS } from "./weather.js";
import {
  pruneWeatherRecordsBefore,
  storeWeatherRecords,
} from "./weather-store.js";
import type { WeatherFetchJobResult, WeatherPointFailure } from "./types.js";

/**
 * OpenWeatherMap weather data-collection job for the FI forecast (issue #73,
 * Phase 1: forward-only collection; no change to any price/forecast response).
 * Mirrors `forecast-job.ts`: fetch the public upstream, store idempotently,
 * then prune beyond the retention window. The OWM boundary degrades rather than
 * throwing, so this job can never break the authoritative price path.
 *
 * Per-point degrade (devils-advocate scope cut): the configured points are
 * fetched and stored INDEPENDENTLY. One point's transient failure must not
 * discard the other point's irreversible issue-time data — that issuance can
 * never be re-fetched once the hour passes. So a failed point is recorded in
 * `failures` and the run reports `partial`; it does not abort the whole run.
 */

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Retention window for stored weather rows (~13 months). Like the Fingrid job,
 * the table ACCUMULATES forward from deploy: each hourly run appends a fresh
 * issuance, building the issue-time forecast history a later leakage-free
 * weather-feature backtest (Phase 3) needs. 400 days covers a full year plus a
 * month of margin so a backtest spanning a complete seasonal cycle always has
 * data, while still bounding storage per the VISION data-footprint principle
 * (2 points × 48 hourly entries × 24 issuances/day × 400 days ≈ 920k rows,
 * trivial on Railway).
 */
export const WEATHER_RETENTION_DAYS = 400;

/**
 * Run the weather fetch job across all configured points. `now` is injectable
 * for testability; defaults to the current instant. Always resolves with a
 * tagged result (full success / partial / total failure) — it never throws.
 */
export const runWeatherFetchJob = async (
  pool: Pool,
  apiKey: string,
  now: Date = new Date(),
): Promise<WeatherFetchJobResult> => {
  let stored = 0;
  let successes = 0;
  const failures: WeatherPointFailure[] = [];

  for (const point of WEATHER_POINTS) {
    const result = await fetchWeather({ apiKey, point, issuedAt: now });
    if (!result.ok) {
      failures.push({ pointId: point.id, reason: result.reason });
      continue;
    }
    stored += await storeWeatherRecords(pool, result.records);
    successes += 1;
  }

  // Every point failed: nothing was stored, so do not prune either — report a
  // total failure and leave the table untouched.
  if (successes === 0) {
    return { status: "failed", failures };
  }

  // Prune issuances older than the retention window so the table stays bounded
  // while still accumulating forward from deploy.
  const pruneCutoff = new Date(
    now.getTime() - WEATHER_RETENTION_DAYS * DAY_MS,
  ).toISOString();
  const pruned = await pruneWeatherRecordsBefore(pool, pruneCutoff);

  if (failures.length > 0) {
    return { status: "partial", stored, pruned, failures };
  }
  return { status: "ok", stored, pruned };
};

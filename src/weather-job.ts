import type { Pool } from "pg";
import { fetchWeather, WEATHER_POINTS } from "./weather.js";
import {
  pruneWeatherDailyRecordsBefore,
  pruneWeatherRecordsBefore,
  storeWeatherDailyRecords,
  storeWeatherRecords,
} from "./weather-store.js";
import type { WeatherFetchJobResult, WeatherPointFailure } from "./types.js";

/**
 * OpenWeatherMap weather data-collection job for the FI forecast. Mirrors
 * `forecast-job.ts`: fetch the public upstream, store idempotently, then prune
 * beyond the retention window. The OWM boundary degrades rather than throwing,
 * so this job can never break the authoritative price path.
 *
 * Per-point degrade: the configured points are fetched and stored
 * INDEPENDENTLY. One point's transient failure must not discard the other
 * point's irreversible issue-time data — that issuance can never be re-fetched
 * once the hour passes. So a failed point is recorded in `failures` and the run
 * reports `partial`; it does not abort the whole run.
 *
 * The DAILY block of the same response is stored on the same principle, one
 * level deeper: it is written after the hourly rows, in its own transaction and
 * its own try/catch, and its failures are reported in a separate `daily`
 * summary. `status`, `stored` and `pruned` remain statements about the HOURLY
 * collection alone. No extra upstream call is made — the daily block rides along
 * in the response the job already fetches.
 */

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Retention window for stored weather rows (~13 months). Keep it above a full
 * year, so a backtest spanning a complete seasonal cycle always has data.
 * Accumulation policy and row math: see `STACK.md §9`.
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
  let dailyStored = 0;
  const dailyFailures: WeatherPointFailure[] = [];

  for (const point of WEATHER_POINTS) {
    const result = await fetchWeather({ apiKey, point, issuedAt: now });

    // HOURLY first, and unchanged: a degraded point is recorded in `failures`
    // and never aborts the run.
    if (result.ok) {
      stored += await storeWeatherRecords(pool, result.records);
      successes += 1;
    } else {
      failures.push({ pointId: point.id, reason: result.reason });
    }

    // Daily block — AFTER the hourly store, in its OWN try/catch and its OWN
    // transaction. The hourly rows are already committed at this point,
    // so nothing here can discard them. `successes` deliberately counts HOURLY
    // successes only, so the "every point failed → do not prune" guard below
    // keeps its meaning, and a daily failure never downgrades `status`.
    //
    // This runs on BOTH branches, NOT after an early `continue` on the hourly
    // failure: the two blocks are parsed independently, so an hourly schema
    // drift can leave a perfectly valid daily block. Skipping it would discard
    // those rows irreversibly (see the note above about re-fetching) and,
    // worse, invisibly — `daily.failures` would stay empty while `daily.stored`
    // silently fell to zero. The isolation holds in both directions.
    try {
      if (result.daily.ok) {
        dailyStored += await storeWeatherDailyRecords(
          pool,
          result.daily.records,
        );
      } else {
        dailyFailures.push({ pointId: point.id, reason: result.daily.reason });
      }
    } catch (err) {
      const reason = err instanceof Error ? err.message : "unknown error";
      dailyFailures.push({ pointId: point.id, reason });
      console.warn(
        `[weather-job] daily store degraded for ${point.id}: ${reason}`,
      );
    }
  }

  const daily = { stored: dailyStored, failures: dailyFailures };

  // Every HOURLY point failed: no fresh hourly data arrived, so do not prune
  // either — report a total failure and leave the tables untouched. The daily
  // summary still rides along: daily rows can exist on this branch (an hourly
  // schema drift with a valid daily block), and a run that stored them must say
  // so. Not pruning for one hour costs nothing; pruning during an upstream
  // outage could cost history.
  if (successes === 0) {
    return { status: "failed", failures, daily };
  }

  // Prune issuances older than the retention window so the table stays bounded
  // while still accumulating forward from deploy.
  const pruneCutoff = new Date(
    now.getTime() - WEATHER_RETENTION_DAYS * DAY_MS,
  ).toISOString();
  const pruned = await pruneWeatherRecordsBefore(pool, pruneCutoff);

  // Same cutoff, same retention window, same point in the run — the daily table
  // shares `WEATHER_RETENTION_DAYS` and gets no constant of its own. Isolated
  // like the daily store: a prune failure must not lose the run's result, and it
  // cannot lose data, because every row it could touch is already committed.
  try {
    await pruneWeatherDailyRecordsBefore(pool, pruneCutoff);
  } catch (err) {
    const reason = err instanceof Error ? err.message : "unknown error";
    console.warn(`[weather-job] daily retention prune degraded: ${reason}`);
  }

  if (failures.length > 0) {
    return { status: "partial", stored, pruned, failures, daily };
  }
  return { status: "ok", stored, pruned, daily };
};

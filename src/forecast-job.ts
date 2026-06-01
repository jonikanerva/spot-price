import type { Pool } from "pg";
import { fetchFingridSeries } from "./fingrid.js";
import {
  pruneFingridRecordsBefore,
  storeFingridRecords,
} from "./fingrid-store.js";
import { FLOOR_HISTORY_DAYS, FORECAST_DAYS } from "./forecast.js";

/**
 * Fingrid grid-data fetch job for the FI forecast. Mirrors `fetch-job.ts`:
 * fetches the four public datasets for [now − HISTORY_DAYS, now + FORECAST_DAYS],
 * upserts them idempotently, then prunes rows outside the needed window to bound
 * table growth. Returns a typed result; failures degrade (the Fingrid boundary
 * never throws), so this job can never break the authoritative price path.
 */

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * History fetched before "now": enough to cover the wind weekly extension
 * (4 weeks) and the floor history (30 days), plus a day of margin. The prune
 * keeps this same window so the table stays roughly this size.
 */
export const HISTORY_DAYS = Math.max(FLOOR_HISTORY_DAYS, 4 * 7) + 1; // 31 days

/** Margin kept before the prune cutoff so the floor window never under-fills. */
const PRUNE_MARGIN_DAYS = 4;

export type ForecastFetchResult =
  | { readonly ok: true; readonly stored: number; readonly pruned: number }
  | { readonly ok: false; readonly reason: string };

/**
 * Run the Fingrid fetch job. `now` is injectable for testability; defaults to
 * the current instant.
 */
export const runForecastFetchJob = async (
  pool: Pool,
  apiKey: string,
  now: Date = new Date(),
): Promise<ForecastFetchResult> => {
  const nowMs = now.getTime();
  const startUtc = new Date(nowMs - HISTORY_DAYS * DAY_MS).toISOString();
  const endUtc = new Date(nowMs + FORECAST_DAYS * DAY_MS).toISOString();

  const result = await fetchFingridSeries({ apiKey, startUtc, endUtc });
  if (!result.ok) {
    return { ok: false, reason: result.reason };
  }

  const stored = await storeFingridRecords(pool, result.records);

  const pruneCutoff = new Date(
    nowMs - (HISTORY_DAYS + PRUNE_MARGIN_DAYS) * DAY_MS,
  ).toISOString();
  const pruned = await pruneFingridRecordsBefore(pool, pruneCutoff);

  return { ok: true, stored, pruned };
};

import type { Pool } from "pg";
import { fetchFingridSeries } from "./fingrid.js";
import {
  pruneFingridRecordsBefore,
  storeFingridRecords,
} from "./fingrid-store.js";
import { FLOOR_HISTORY_DAYS, FORECAST_DAYS } from "./forecast.js";

/**
 * Fingrid grid-data fetch job for the FI forecast. Mirrors `fetch-job.ts`:
 * fetches the four public datasets for [now − HISTORY_DAYS, now + FORECAST_DAYS]
 * and upserts them idempotently, then prunes rows older than the retention
 * window to bound table growth. Returns a typed result; failures degrade (the
 * Fingrid boundary never throws), so this job can never break the authoritative
 * price path.
 *
 * The FETCH-back window (~31 days) and the RETENTION window (~2 years) are
 * decoupled on purpose: Fingrid only serves data forward from now, so fetching
 * further back can never backfill missed history — there is no point widening
 * the fetch window. Retention, by contrast, is kept long so the table
 * ACCUMULATES grid history from deploy onward, giving future forecast phases
 * (conformal intervals, tree models) seasonal data to backtest against. History
 * therefore fills forward over time; it is not retroactive, and forecast
 * quality for those phases ramps up as the window fills.
 */

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * History fetched before "now": enough to cover the wind weekly extension
 * (4 weeks) and the floor history (30 days), plus a day of margin. Unchanged —
 * Fingrid serves data only forward from now, so a wider fetch window cannot
 * backfill anything.
 */
export const HISTORY_DAYS = Math.max(FLOOR_HISTORY_DAYS, 4 * 7) + 1; // 31 days

/**
 * Retention window for stored Fingrid rows (~2 years). Forward-looking
 * accumulation for future forecast phases (Phase 2 conformal, Phase 3 trees):
 * the table grows from deploy date as the hourly job appends fresh quarters,
 * building up the seasonal history those phases need to backtest against.
 *
 * Bounded to cap storage per the VISION data-footprint principle: 4 datasets ×
 * 96 quarters/day × 730 days ≈ 280k rows ≈ ~14 MB plus index — trivial on
 * Railway. This is well clear of the 31-day fetch + 30-day floor windows, so
 * the live forecast path never under-fills.
 */
export const RETENTION_DAYS = 730;

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

  // Prune only rows older than the retention window, NOT the fetch window — the
  // table accumulates ~2 years of history forward from deploy for future phases.
  const pruneCutoff = new Date(nowMs - RETENTION_DAYS * DAY_MS).toISOString();
  const pruned = await pruneFingridRecordsBefore(pool, pruneCutoff);

  return { ok: true, stored, pruned };
};

import type { Pool } from "pg";
import {
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_FORECAST,
  fetchFingridSeries,
} from "./fingrid.js";
import {
  pruneFingridForecastVintagesBefore,
  pruneFingridRecordsBefore,
  storeFingridForecastVintages,
  storeFingridRecords,
} from "./fingrid-store.js";
import { FLOOR_HISTORY_DAYS, FORECAST_DAYS } from "./forecast.js";

/**
 * Fingrid grid-data fetch job for the FI forecast. Mirrors `fetch-job.ts`:
 * fetches the four public datasets for [now − HISTORY_DAYS, now + FORECAST_DAYS],
 * then partitions them by class to ONE home each — ACTUALS (75/124) are
 * upsert-latest in `fingrid_actuals`; FORECASTS (245/165) are archived
 * append-only per issuance in `fingrid_forecasts`. Each store prunes
 * rows older than its retention window to bound growth. Returns a typed result;
 * failures degrade (the Fingrid boundary never throws), so this job can never
 * break the authoritative price path.
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
 * Retention window for the `fingrid_actuals` ACTUAL rows (~2 years). Keep it
 * well clear of the 31-day fetch window and the 30-day floor window, so the
 * live forecast path never under-fills. Row math and accumulation policy: see
 * `STACK.md §9`.
 */
export const RETENTION_DAYS = 730;

/**
 * Retention for the per-issuance forecast vintages. Set independently of
 * `RETENTION_DAYS` — see `STACK.md §5`.
 */
export const VINTAGE_RETENTION_DAYS = 180;

export type ForecastFetchResult =
  | {
      readonly ok: true;
      readonly stored: number;
      readonly pruned: number;
      /** Forecast vintages inserted this run (append-only per issuance). */
      readonly vintageStored: number;
      /** Vintages pruned this run (issuances older than retention). */
      readonly vintagePruned: number;
      /** Set when the vintage write/prune degraded but the authoritative upsert succeeded. */
      readonly vintageDegradedReason?: string;
    }
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

  // Partition the fetch result by dataset class. Each class has ONE home:
  // actuals (75/124) -> upsert-latest in `fingrid_actuals`; forecasts (245/165)
  // -> append-only per issuance in `fingrid_forecasts`. Forecasts are
  // deliberately NOT written to `fingrid_actuals`.
  const isForecastDataset = (datasetId: number): boolean =>
    datasetId === DATASET_WIND_FORECAST ||
    datasetId === DATASET_CONSUMPTION_FORECAST;
  const actualRecords = result.records.filter(
    (r) => !isForecastDataset(r.datasetId),
  );

  // Step 1 — authoritative upsert of the ACTUAL datasets only. Commits FIRST
  // and on its own; the vintage write below is a separate transaction.
  // `stored` now counts actuals only.
  const stored = await storeFingridRecords(pool, actualRecords);

  // Prune only rows older than the retention window, NOT the fetch window — the
  // table accumulates ~2 years of actual history forward from deploy.
  const pruneCutoff = new Date(nowMs - RETENTION_DAYS * DAY_MS).toISOString();
  const pruned = await pruneFingridRecordsBefore(pool, pruneCutoff);

  // Step 2 — per-issuance vintage archival of the FORECAST datasets (issue #78),
  // in its OWN try/catch and its OWN transaction. Per STACK §9 the forecast path
  // must never affect the authoritative actuals upsert above: a vintage failure
  // here degrades (logged + reported) and can never roll back or abort step 1.
  // `storeFingridForecastVintages` filters internally — to 245/165, and (issue
  // #90) to targets no older than `VINTAGE_BACKFILL_HOURS` before the issuance —
  // so passing the full 34-day fetch result is safe and archives only the
  // forecast part of it. `issuedAt` is the job's `now`, hour-truncated to UTC (a
  // fetch-time proxy for true issuance, ±1h of jitter) so re-runs within the
  // same hour stay idempotent against the (dataset_id, issued_at, start_time) PK.
  const issuedAt = new Date(
    Date.UTC(
      now.getUTCFullYear(),
      now.getUTCMonth(),
      now.getUTCDate(),
      now.getUTCHours(),
    ),
  ).toISOString();
  let vintageStored = 0;
  let vintagePruned = 0;
  let vintageDegradedReason: string | undefined;
  try {
    vintageStored = await storeFingridForecastVintages(
      pool,
      issuedAt,
      result.records,
    );
    const vintageCutoff = new Date(
      nowMs - VINTAGE_RETENTION_DAYS * DAY_MS,
    ).toISOString();
    vintagePruned = await pruneFingridForecastVintagesBefore(
      pool,
      vintageCutoff,
    );
  } catch (err) {
    vintageDegradedReason =
      err instanceof Error ? err.message : "unknown error";
    console.warn(
      `Fingrid forecast-vintage archival degraded: ${vintageDegradedReason}`,
    );
  }

  return vintageDegradedReason === undefined
    ? { ok: true, stored, pruned, vintageStored, vintagePruned }
    : {
        ok: true,
        stored,
        pruned,
        vintageStored,
        vintagePruned,
        vintageDegradedReason,
      };
};

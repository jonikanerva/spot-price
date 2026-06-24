import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import {
  getFingridForecastVintagesLatest,
  getFingridRecordsByRange,
  storeFingridForecastVintages,
  storeFingridRecords,
} from "./fingrid-store.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "./fingrid.js";
import {
  HISTORY_DAYS,
  RETENTION_DAYS,
  VINTAGE_RETENTION_DAYS,
} from "./forecast-job.js";
import type { FingridRecord } from "./types.js";

const DAY_MS = 24 * 60 * 60 * 1000;
const NOW = new Date("2026-04-15T12:00:00.000Z");

const record = (ms: number, value: number): FingridRecord => ({
  datasetId: DATASET_WIND_FORECAST,
  startTime: new Date(ms).toISOString(),
  endTime: new Date(ms + 15 * 60 * 1000).toISOString(),
  value,
});

const recordOf = (
  datasetId: number,
  ms: number,
  value: number,
): FingridRecord => ({
  datasetId,
  startTime: new Date(ms).toISOString(),
  endTime: new Date(ms + 15 * 60 * 1000).toISOString(),
  value,
});

/** Raw row count in the vintage table for a dataset, regardless of issuance. */
const countVintageRows = async (
  pool: Pool,
  datasetId: number,
): Promise<number> => {
  const { rows } = await pool.query<{ n: string }>(
    `SELECT COUNT(*) AS n FROM fingrid_forecasts WHERE dataset_id = $1`,
    [datasetId],
  );
  return parseInt(rows[0]?.n ?? "0", 10);
};

describe("fingrid-store", () => {
  let pool: Pool;
  beforeEach(async () => {
    pool = await initTestDatabase();
  });
  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("upserts idempotently and reads back by range", async () => {
    const ms = NOW.getTime();
    await storeFingridRecords(pool, [recordOf(DATASET_WIND_ACTUAL, ms, 100)]);
    // Re-store with a new value for the same key — should update, not duplicate.
    await storeFingridRecords(pool, [recordOf(DATASET_WIND_ACTUAL, ms, 200)]);

    const rows = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    expect(rows).toHaveLength(1);
    expect(rows[0]?.value).toBe(200);
  });
});

describe("fingrid forecast vintages", () => {
  let pool: Pool;
  beforeEach(async () => {
    pool = await initTestDatabase();
  });
  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("archives a new row per issuance for the same (dataset, start) — append-only, the inverse of the upsert", async () => {
    const ms = NOW.getTime();
    const firstIssuance = new Date(
      NOW.getTime() - 2 * 60 * 60 * 1000,
    ).toISOString();
    const secondIssuance = NOW.toISOString();

    const a = await storeFingridForecastVintages(pool, firstIssuance, [
      record(ms, 100),
    ]);
    const b = await storeFingridForecastVintages(pool, secondIssuance, [
      record(ms, 200),
    ]);
    expect(a).toBe(1);
    expect(b).toBe(1);

    // Both issuances persist — the issue-time forecast is preserved, never
    // overwritten to the latest issuance.
    expect(await countVintageRows(pool, DATASET_WIND_FORECAST)).toBe(2);
  });

  it("getFingridForecastVintagesLatest returns EXACTLY ONE row per target (newest issuance) — guards the bucketRecords averaging trap", async () => {
    const ms = NOW.getTime();
    // THREE issuances for the SAME target: a stale +44h-old (10), a mid one
    // (20), and the freshest +1h-old (30). The live read MUST collapse to the
    // newest only. If it returned all three, `bucketRecords` (forecast.ts) would
    // silently MEAN them to 20 — a wrong value with no error.
    const stale = new Date(NOW.getTime() - 44 * 60 * 60 * 1000).toISOString();
    const mid = new Date(NOW.getTime() - 12 * 60 * 60 * 1000).toISOString();
    const fresh = new Date(NOW.getTime() - 1 * 60 * 60 * 1000).toISOString();
    await storeFingridForecastVintages(pool, stale, [record(ms, 10)]);
    await storeFingridForecastVintages(pool, fresh, [record(ms, 30)]);
    await storeFingridForecastVintages(pool, mid, [record(ms, 20)]);

    // All three are stored…
    expect(await countVintageRows(pool, DATASET_WIND_FORECAST)).toBe(3);

    // …but the live read returns exactly one row, the newest issuance's value.
    const latest = await getFingridForecastVintagesLatest(
      pool,
      DATASET_WIND_FORECAST,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    expect(latest).toHaveLength(1);
    expect(latest[0]?.value).toBe(30);
  });

  it("is idempotent on the same issuance (ON CONFLICT DO NOTHING, 0 inserts)", async () => {
    const ms = NOW.getTime();
    const issuance = NOW.toISOString();

    const first = await storeFingridForecastVintages(pool, issuance, [
      record(ms, 100),
    ]);
    // Same (dataset, issuance, start) with a different value — must NOT insert
    // and must NOT overwrite the original.
    const second = await storeFingridForecastVintages(pool, issuance, [
      record(ms, 999),
    ]);
    expect(first).toBe(1);
    expect(second).toBe(0);

    expect(await countVintageRows(pool, DATASET_WIND_FORECAST)).toBe(1);
    const latest = await getFingridForecastVintagesLatest(
      pool,
      DATASET_WIND_FORECAST,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    expect(latest[0]?.value).toBe(100);
  });

  it("never archives actual datasets (75/124) into the vintage table", async () => {
    const ms = NOW.getTime();
    const issuance = NOW.toISOString();

    // Forecast + both actuals handed in together; only the forecast persists.
    const inserted = await storeFingridForecastVintages(pool, issuance, [
      record(ms, 100), // 245 wind forecast
      recordOf(DATASET_WIND_ACTUAL, ms, 500), // 75 actual — must be ignored
      recordOf(DATASET_CONSUMPTION_ACTUAL, ms, 8000), // 124 actual — must be ignored
    ]);
    expect(inserted).toBe(1);

    expect(await countVintageRows(pool, DATASET_WIND_ACTUAL)).toBe(0);
    expect(await countVintageRows(pool, DATASET_CONSUMPTION_ACTUAL)).toBe(0);
    expect(await countVintageRows(pool, DATASET_WIND_FORECAST)).toBe(1);
  });

  it("leaves the fingrid_actuals upsert for actuals unchanged", async () => {
    const ms = NOW.getTime();
    // The authoritative series still upserts the latest actual value.
    await storeFingridRecords(pool, [recordOf(DATASET_WIND_ACTUAL, ms, 500)]);
    await storeFingridRecords(pool, [recordOf(DATASET_WIND_ACTUAL, ms, 600)]);

    const rows = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    expect(rows).toHaveLength(1);
    expect(rows[0]?.value).toBe(600);
  });
});

describe("runForecastFetchJob", () => {
  let pool: Pool;
  beforeEach(async () => {
    pool = await initTestDatabase();
  });
  afterEach(async () => {
    await closeDatabase(pool);
    vi.restoreAllMocks();
  });

  it("prunes only ACTUAL rows older than the 2-year retention window, retaining accumulated history", async () => {
    // A row beyond the retention window — must be pruned.
    const expiredMs = NOW.getTime() - (RETENTION_DAYS + 10) * DAY_MS;
    // A row WITHIN retention but well older than the 31-day fetch window —
    // accumulating this history is the whole point, so it must be RETAINED.
    const accumulatedMs = NOW.getTime() - (HISTORY_DAYS + 100) * DAY_MS;
    expect(accumulatedMs).toBeGreaterThan(
      NOW.getTime() - RETENTION_DAYS * DAY_MS,
    );
    await storeFingridRecords(pool, [
      recordOf(DATASET_WIND_ACTUAL, expiredMs, 999),
      recordOf(DATASET_WIND_ACTUAL, accumulatedMs, 555),
    ]);

    const freshMs = NOW.getTime();
    const fingrid = await import("./fingrid.js");
    vi.spyOn(fingrid, "fetchFingridSeries").mockResolvedValue({
      ok: true,
      records: [recordOf(DATASET_WIND_ACTUAL, freshMs, 111)],
    });

    const { runForecastFetchJob } = await import("./forecast-job.js");
    const result = await runForecastFetchJob(pool, "test-key", NOW);

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.stored).toBe(1);
      // Only the expired row is pruned; the accumulated row is kept.
      expect(result.pruned).toBe(1);
    }

    // Read across the full span: the expired row is gone, while both the
    // accumulated (older than the fetch window) and the fresh row remain.
    const remaining = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(expiredMs - DAY_MS).toISOString(),
      new Date(freshMs + DAY_MS).toISOString(),
    );
    const values = remaining.map((r) => r.value).sort((a, b) => a - b);
    expect(values).toEqual([111, 555]);
  });

  it("single-home partition: forecasts go ONLY to the vintage table, actuals ONLY to fingrid_actuals, counts correct", async () => {
    const target = NOW.getTime();
    const issuedHour = new Date(
      Date.UTC(
        NOW.getUTCFullYear(),
        NOW.getUTCMonth(),
        NOW.getUTCDate(),
        NOW.getUTCHours(),
      ),
    ).toISOString();

    // Pre-seed one EARLIER vintage for the same target (an accumulated, within-
    // retention issuance) and one EXPIRED vintage (beyond 180d) to prune.
    const earlierIssuance = new Date(NOW.getTime() - 3 * DAY_MS).toISOString();
    const expiredIssuance = new Date(
      NOW.getTime() - (VINTAGE_RETENTION_DAYS + 10) * DAY_MS,
    ).toISOString();
    await storeFingridForecastVintages(pool, earlierIssuance, [
      record(target, 100),
    ]);
    await storeFingridForecastVintages(pool, expiredIssuance, [
      record(target, 50),
    ]);

    // One fetch result carrying a fresh forecast value (245) and an actual (75)
    // for the same target.
    const fingrid = await import("./fingrid.js");
    vi.spyOn(fingrid, "fetchFingridSeries").mockResolvedValue({
      ok: true,
      records: [
        record(target, 200),
        recordOf(DATASET_WIND_ACTUAL, target, 700),
      ],
    });

    const { runForecastFetchJob } = await import("./forecast-job.js");
    const result = await runForecastFetchJob(pool, "test-key", NOW);

    expect(result.ok).toBe(true);
    if (result.ok) {
      // `stored` counts ACTUALS only — the forecast did NOT touch fingrid_actuals.
      expect(result.stored).toBe(1);
      expect(result.vintageStored).toBe(1); // only the forecast vintage archived
      expect(result.vintagePruned).toBe(1); // only the expired issuance pruned
      expect(result.vintageDegradedReason).toBeUndefined();
    }

    // fingrid_actuals holds the ACTUAL only — the forecast (245) was NOT written.
    const seriesForecast = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_FORECAST,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(seriesForecast).toHaveLength(0);

    const seriesActual = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(seriesActual).toHaveLength(1);
    expect(seriesActual[0]?.value).toBe(700);

    // The vintage table ACCUMULATES: the earlier issuance (100) and this run's
    // fresh issuance (200) both remain; the expired one (50) was pruned.
    expect(await countVintageRows(pool, DATASET_WIND_FORECAST)).toBe(2);
    // The live read returns the newest issuance for the target.
    const latest = await getFingridForecastVintagesLatest(
      pool,
      DATASET_WIND_FORECAST,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(latest).toHaveLength(1);
    expect(latest[0]?.value).toBe(200);

    // The run's issuance was recorded hour-truncated.
    const issuedRows = await pool.query<{ issued_at: string }>(
      `SELECT DISTINCT issued_at FROM fingrid_forecasts
       WHERE issued_at = $1`,
      [issuedHour],
    );
    expect(issuedRows.rows).toHaveLength(1);

    // The actual never reached the vintage table.
    expect(await countVintageRows(pool, DATASET_WIND_ACTUAL)).toBe(0);
  });

  it("vintage failure is isolated: actuals still commit, ok:true, vintageDegradedReason set", async () => {
    const target = NOW.getTime();
    const fingrid = await import("./fingrid.js");
    vi.spyOn(fingrid, "fetchFingridSeries").mockResolvedValue({
      ok: true,
      records: [
        record(target, 200),
        recordOf(DATASET_WIND_ACTUAL, target, 700),
      ],
    });

    // Make the vintage write throw AFTER the authoritative actuals upsert.
    const store = await import("./fingrid-store.js");
    vi.spyOn(store, "storeFingridForecastVintages").mockRejectedValue(
      new Error("vintage table blew up"),
    );
    // Silence the expected degrade warning.
    vi.spyOn(console, "warn").mockImplementation(() => undefined);

    const { runForecastFetchJob } = await import("./forecast-job.js");
    const result = await runForecastFetchJob(pool, "test-key", NOW);

    // The job still succeeds — a vintage failure can never abort actuals.
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.stored).toBe(1); // actual committed
      expect(result.vintageStored).toBe(0);
      expect(result.vintageDegradedReason).toContain("vintage table blew up");
    }

    // The authoritative actual is durably committed despite the vintage failure.
    const seriesActual = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(seriesActual).toHaveLength(1);
    expect(seriesActual[0]?.value).toBe(700);
  });

  it("returns a degraded result without throwing when Fingrid degrades", async () => {
    const fingrid = await import("./fingrid.js");
    vi.spyOn(fingrid, "fetchFingridSeries").mockResolvedValue({
      ok: false,
      records: [],
      reason: "Fingrid auth failed (HTTP 401)",
    });

    const { runForecastFetchJob } = await import("./forecast-job.js");
    const result = await runForecastFetchJob(pool, "bad-key", NOW);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("auth");
    }
  });
});

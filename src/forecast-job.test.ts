import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import {
  getFingridForecastVintagesByRange,
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
    await storeFingridRecords(pool, [record(ms, 100)]);
    // Re-store with a new value for the same key — should update, not duplicate.
    await storeFingridRecords(pool, [record(ms, 200)]);

    const rows = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_FORECAST,
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

    const rows = await getFingridForecastVintagesByRange(
      pool,
      DATASET_WIND_FORECAST,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    // Two rows — one per issuance — the issue-time forecast is preserved, never
    // overwritten to the latest issuance.
    expect(rows).toHaveLength(2);
    expect(rows.map((r) => r.value).sort((x, y) => x - y)).toEqual([100, 200]);
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

    const rows = await getFingridForecastVintagesByRange(
      pool,
      DATASET_WIND_FORECAST,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    expect(rows).toHaveLength(1);
    expect(rows[0]?.value).toBe(100);
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

    const windActual = await getFingridForecastVintagesByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    const consumptionActual = await getFingridForecastVintagesByRange(
      pool,
      DATASET_CONSUMPTION_ACTUAL,
      new Date(ms - DAY_MS).toISOString(),
      new Date(ms + DAY_MS).toISOString(),
    );
    expect(windActual).toHaveLength(0);
    expect(consumptionActual).toHaveLength(0);
  });

  it("leaves the fingrid_series upsert for actuals unchanged", async () => {
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

  it("prunes only rows older than the 2-year retention window, retaining accumulated history", async () => {
    // A row beyond the retention window — must be pruned.
    const expiredMs = NOW.getTime() - (RETENTION_DAYS + 10) * DAY_MS;
    // A row WITHIN retention but well older than the 31-day fetch window —
    // accumulating this history is the whole point, so it must be RETAINED.
    const accumulatedMs = NOW.getTime() - (HISTORY_DAYS + 100) * DAY_MS;
    expect(accumulatedMs).toBeGreaterThan(
      NOW.getTime() - RETENTION_DAYS * DAY_MS,
    );
    await storeFingridRecords(pool, [
      record(expiredMs, 999),
      record(accumulatedMs, 555),
    ]);

    const freshMs = NOW.getTime();
    const fingrid = await import("./fingrid.js");
    vi.spyOn(fingrid, "fetchFingridSeries").mockResolvedValue({
      ok: true,
      records: [record(freshMs, 111)],
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
      DATASET_WIND_FORECAST,
      new Date(expiredMs - DAY_MS).toISOString(),
      new Date(freshMs + DAY_MS).toISOString(),
    );
    const values = remaining.map((r) => r.value).sort((a, b) => a - b);
    expect(values).toEqual([111, 555]);
  });

  it("writes both tables from one fetch: fingrid_series stays latest-only, vintages accumulate, and the vintage prune drops only old issuances", async () => {
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

    // One fetch result carrying a fresh forecast value for the SAME target plus
    // an actual (which must reach fingrid_series but never the vintage table).
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
      expect(result.stored).toBe(2); // both records upserted to fingrid_series
      expect(result.vintageStored).toBe(1); // only the forecast vintage archived
      expect(result.vintagePruned).toBe(1); // only the expired issuance pruned
      expect(result.vintageDegradedReason).toBeUndefined();
    }

    // fingrid_series is latest-only: one row for the forecast target, holding
    // the freshly upserted value.
    const seriesRows = await getFingridRecordsByRange(
      pool,
      DATASET_WIND_FORECAST,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(seriesRows).toHaveLength(1);
    expect(seriesRows[0]?.value).toBe(200);

    // The vintage table ACCUMULATES: the earlier issuance (100) and this run's
    // fresh issuance (200) both remain; the expired one (50) was pruned.
    const vintageRows = await getFingridForecastVintagesByRange(
      pool,
      DATASET_WIND_FORECAST,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(vintageRows.map((r) => r.value).sort((a, b) => a - b)).toEqual([
      100, 200,
    ]);

    // The run's issuance was recorded hour-truncated.
    const allVintages = await pool.query<{ issued_at: string }>(
      `SELECT DISTINCT issued_at FROM fingrid_forecast_vintages
       WHERE issued_at = $1`,
      [issuedHour],
    );
    expect(allVintages.rows).toHaveLength(1);

    // The actual reached fingrid_series but NOT the vintage table.
    const vintageActuals = await getFingridForecastVintagesByRange(
      pool,
      DATASET_WIND_ACTUAL,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(vintageActuals).toHaveLength(0);
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

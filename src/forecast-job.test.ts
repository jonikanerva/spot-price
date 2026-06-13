import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import {
  getFingridRecordsByRange,
  storeFingridRecords,
} from "./fingrid-store.js";
import { DATASET_WIND_FORECAST } from "./fingrid.js";
import { HISTORY_DAYS, RETENTION_DAYS } from "./forecast-job.js";
import type { FingridRecord } from "./types.js";

const DAY_MS = 24 * 60 * 60 * 1000;
const NOW = new Date("2026-04-15T12:00:00.000Z");

const record = (ms: number, value: number): FingridRecord => ({
  datasetId: DATASET_WIND_FORECAST,
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

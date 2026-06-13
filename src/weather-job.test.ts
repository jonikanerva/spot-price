import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import {
  getWeatherRecordsByRange,
  storeWeatherRecords,
} from "./weather-store.js";
import { HELSINKI, VAASA, WEATHER_POINTS } from "./weather.js";
import { WEATHER_RETENTION_DAYS } from "./weather-job.js";
import type { WeatherFetchResult, WeatherRecord } from "./types.js";

const DAY_MS = 24 * 60 * 60 * 1000;
const NOW = new Date("2026-06-13T13:00:00.000Z");
const POINT_A = HELSINKI.id; // helsinki
const POINT_B = VAASA.id; // vaasa

const record = (
  pointId: string,
  issuedAtMs: number,
  targetMs: number,
  temp: number,
): WeatherRecord => ({
  pointId,
  issuedAt: new Date(issuedAtMs).toISOString(),
  targetTime: new Date(targetMs).toISOString(),
  temp,
  clouds: 50,
  uvi: 1,
  windSpeed: 4,
  windDeg: 180,
});

const okResult = (records: readonly WeatherRecord[]): WeatherFetchResult => ({
  ok: true,
  records,
});

describe("weather-store", () => {
  let pool: Pool;
  beforeEach(async () => {
    pool = await initTestDatabase();
  });
  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("archives a new row per issuance for the same target (leakage-free property)", async () => {
    const target = NOW.getTime() + DAY_MS;
    const firstIssuance = NOW.getTime() - 2 * 60 * 60 * 1000;
    const secondIssuance = NOW.getTime();

    const a = await storeWeatherRecords(pool, [
      record(POINT_A, firstIssuance, target, 10),
    ]);
    const b = await storeWeatherRecords(pool, [
      record(POINT_A, secondIssuance, target, 12),
    ]);
    expect(a).toBe(1);
    expect(b).toBe(1);

    const rows = await getWeatherRecordsByRange(
      pool,
      POINT_A,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    // TWO rows: one per issuance — the issue-time forecast is preserved, never
    // overwritten to the latest issuance.
    expect(rows).toHaveLength(2);
    expect(rows.map((r) => r.temp).sort((x, y) => x - y)).toEqual([10, 12]);
  });

  it("is idempotent on the same key (ON CONFLICT DO NOTHING, not overwrite)", async () => {
    const issued = NOW.getTime();
    const target = NOW.getTime() + DAY_MS;

    const first = await storeWeatherRecords(pool, [
      record(POINT_A, issued, target, 10),
    ]);
    // Same (point, issuance, target) with a different value — must NOT insert
    // and must NOT overwrite the original.
    const second = await storeWeatherRecords(pool, [
      record(POINT_A, issued, target, 99),
    ]);
    expect(first).toBe(1);
    expect(second).toBe(0);

    const rows = await getWeatherRecordsByRange(
      pool,
      POINT_A,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(rows).toHaveLength(1);
    expect(rows[0]?.temp).toBe(10);
  });
});

describe("runWeatherFetchJob", () => {
  let pool: Pool;
  beforeEach(async () => {
    pool = await initTestDatabase();
  });
  afterEach(async () => {
    await closeDatabase(pool);
    vi.restoreAllMocks();
  });

  it("prunes issuances older than retention, retaining within-retention history older than 48h", async () => {
    const target = NOW.getTime() + DAY_MS;
    // An issuance beyond the retention window — must be pruned.
    const expiredIssuance =
      NOW.getTime() - (WEATHER_RETENTION_DAYS + 10) * DAY_MS;
    // An issuance within retention but well older than 48h — accumulating this
    // history is the whole point, so it must be RETAINED.
    const accumulatedIssuance = NOW.getTime() - 30 * DAY_MS;
    expect(accumulatedIssuance).toBeGreaterThan(
      NOW.getTime() - WEATHER_RETENTION_DAYS * DAY_MS,
    );
    await storeWeatherRecords(pool, [
      record(POINT_A, expiredIssuance, target, 1),
      record(POINT_A, accumulatedIssuance, target, 2),
    ]);

    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockImplementation(
      ({ point, issuedAt }) =>
        Promise.resolve(
          okResult([record(point.id, issuedAt.getTime(), target, 3)]),
        ),
    );

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "test-key", NOW);

    expect(result.status).toBe("ok");
    if (result.status === "ok") {
      // Two points each store one fresh row.
      expect(result.stored).toBe(WEATHER_POINTS.length);
      // Only the expired issuance is pruned.
      expect(result.pruned).toBe(1);
    }

    const remaining = await getWeatherRecordsByRange(
      pool,
      POINT_A,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    // Expired gone; accumulated (2) and fresh (3) remain for point A.
    expect(remaining.map((r) => r.temp).sort((x, y) => x - y)).toEqual([2, 3]);
  });

  it("degrades per point: one point fails, the other is still stored, no throw", async () => {
    const target = NOW.getTime() + DAY_MS;

    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockImplementation(
      ({ point, issuedAt }) => {
        if (point.id === POINT_A) {
          return Promise.resolve<WeatherFetchResult>({
            ok: false,
            records: [],
            reason: "OpenWeatherMap auth failed (HTTP 401)",
          });
        }
        return Promise.resolve(
          okResult([record(point.id, issuedAt.getTime(), target, 7)]),
        );
      },
    );

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "test-key", NOW);

    expect(result.status).toBe("partial");
    if (result.status === "partial") {
      expect(result.stored).toBe(1);
      expect(result.failures.map((f) => f.pointId)).toEqual([POINT_A]);
      expect(result.failures[0]?.reason).toContain("auth");
    }

    // Point B's irreversible issue-time data survived the other point's failure.
    const stored = await getWeatherRecordsByRange(
      pool,
      POINT_B,
      new Date(target - DAY_MS).toISOString(),
      new Date(target + DAY_MS).toISOString(),
    );
    expect(stored).toHaveLength(1);
    expect(stored[0]?.temp).toBe(7);
  });

  it("reports total failure without throwing when every point degrades", async () => {
    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockResolvedValue({
      ok: false,
      records: [],
      reason: "OpenWeatherMap request timed out",
    });

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "bad-key", NOW);
    expect(result.status).toBe("failed");
    if (result.status === "failed") {
      expect(result.failures).toHaveLength(WEATHER_POINTS.length);
    }
  });
});

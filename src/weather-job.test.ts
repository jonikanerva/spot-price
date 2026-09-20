import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import {
  getWeatherRecordsByRange,
  storeWeatherDailyRecords,
  storeWeatherRecords,
} from "./weather-store.js";
import { HELSINKI, VAASA, WEATHER_POINTS } from "./weather.js";
import { WEATHER_RETENTION_DAYS } from "./weather-job.js";
import type {
  WeatherDailyRecord,
  WeatherFetchResult,
  WeatherRecord,
} from "./types.js";

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

const dailyRecord = (
  pointId: string,
  issuedAtMs: number,
  targetDate: string,
  tempDay: number,
): WeatherDailyRecord => ({
  pointId,
  issuedAt: new Date(issuedAtMs).toISOString(),
  targetDate,
  targetDt: `${targetDate}T09:00:00.000Z`,
  tempMorn: 9,
  tempDay,
  tempEve: 13,
  tempNight: 10,
  tempMin: 8,
  tempMax: 16,
  clouds: 42,
  uvi: 4.2,
  sunrise: `${targetDate}T01:03:00.000Z`,
  sunset: `${targetDate}T19:48:00.000Z`,
});

const okResult = (
  records: readonly WeatherRecord[],
  daily: readonly WeatherDailyRecord[] = [],
): WeatherFetchResult => ({
  ok: true,
  records,
  daily: { ok: true, records: daily },
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

/** Raw daily rows for a point, newest issuance last. */
const readDailyRows = async (
  pool: Pool,
  pointId: string,
): Promise<{ issued_at: string; target_date: string; temp_day: number }[]> => {
  const { rows } = await pool.query<{
    issued_at: string;
    target_date: string;
    temp_day: number;
  }>(
    `SELECT issued_at, target_date::text AS target_date, temp_day
     FROM weather_daily_forecasts
     WHERE point_id = $1
     ORDER BY target_date, issued_at`,
    [pointId],
  );
  return rows;
};

describe("weather-store (daily)", () => {
  let pool: Pool;
  beforeEach(async () => {
    pool = await initTestDatabase();
  });
  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("archives a new row per issuance for the same target date (leakage-free property)", async () => {
    const firstIssuance = NOW.getTime() - 2 * 60 * 60 * 1000;
    const secondIssuance = NOW.getTime();

    const a = await storeWeatherDailyRecords(pool, [
      dailyRecord(POINT_A, firstIssuance, "2026-06-15", 15),
    ]);
    const b = await storeWeatherDailyRecords(pool, [
      dailyRecord(POINT_A, secondIssuance, "2026-06-15", 17),
    ]);
    expect(a).toBe(1);
    expect(b).toBe(1);

    // TWO rows for one target date — what the forecast said at each issue time.
    const rows = await readDailyRows(pool, POINT_A);
    expect(rows).toHaveLength(2);
    expect(rows.map((r) => r.temp_day).sort((x, y) => x - y)).toEqual([15, 17]);
    expect(rows.every((r) => r.target_date === "2026-06-15")).toBe(true);
  });

  it("is idempotent on the same key (ON CONFLICT DO NOTHING, not overwrite)", async () => {
    const issued = NOW.getTime();

    const first = await storeWeatherDailyRecords(pool, [
      dailyRecord(POINT_A, issued, "2026-06-15", 15),
    ]);
    // Same (point, issuance, target date) with a different value — must NOT
    // insert and must NOT overwrite the original.
    const second = await storeWeatherDailyRecords(pool, [
      dailyRecord(POINT_A, issued, "2026-06-15", 99),
    ]);
    expect(first).toBe(1);
    expect(second).toBe(0);

    const rows = await readDailyRows(pool, POINT_A);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.temp_day).toBe(15);
  });

  it("stores a row whose solar bounds are absent (polar day / polar night)", async () => {
    const polar: WeatherDailyRecord = {
      ...dailyRecord(POINT_A, NOW.getTime(), "2026-06-15", 15),
      sunrise: null,
      sunset: null,
    };
    expect(await storeWeatherDailyRecords(pool, [polar])).toBe(1);

    const { rows } = await pool.query<{
      sunrise: string | null;
      sunset: string | null;
    }>(`SELECT sunrise, sunset FROM weather_daily_forecasts`);
    expect(rows[0]?.sunrise).toBeNull();
    expect(rows[0]?.sunset).toBeNull();
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
            daily: {
              ok: false,
              records: [],
              reason: "OpenWeatherMap auth failed (HTTP 401)",
            },
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

  it("stores the daily block and prunes daily issuances on the same retention cutoff", async () => {
    const target = NOW.getTime() + DAY_MS;
    const expiredIssuance =
      NOW.getTime() - (WEATHER_RETENTION_DAYS + 10) * DAY_MS;
    const accumulatedIssuance = NOW.getTime() - 30 * DAY_MS;
    await storeWeatherDailyRecords(pool, [
      dailyRecord(POINT_A, expiredIssuance, "2026-06-15", 1),
      dailyRecord(POINT_A, accumulatedIssuance, "2026-06-15", 2),
    ]);

    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockImplementation(
      ({ point, issuedAt }) =>
        Promise.resolve(
          okResult(
            [record(point.id, issuedAt.getTime(), target, 3)],
            [dailyRecord(point.id, issuedAt.getTime(), "2026-06-15", 3)],
          ),
        ),
    );

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "test-key", NOW);

    expect(result.status).toBe("ok");
    if (result.status === "ok") {
      expect(result.daily.stored).toBe(WEATHER_POINTS.length);
      expect(result.daily.failures).toHaveLength(0);
    }

    // The expired issuance is gone; the accumulated one and this run's fresh one
    // remain — the daily table accumulates exactly like the hourly one.
    const rows = await readDailyRows(pool, POINT_A);
    expect(rows.map((r) => r.temp_day).sort((x, y) => x - y)).toEqual([2, 3]);
  });

  it("ISOLATION: one point's broken daily block keeps every hourly row and the other point's daily rows", async () => {
    const target = NOW.getTime() + DAY_MS;

    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockImplementation(
      ({ point, issuedAt }) => {
        if (point.id === POINT_A) {
          return Promise.resolve<WeatherFetchResult>({
            ok: true,
            records: [record(point.id, issuedAt.getTime(), target, 5)],
            daily: {
              ok: false,
              records: [],
              reason: "OpenWeatherMap daily block failed schema validation",
            },
          });
        }
        return Promise.resolve(
          okResult(
            [record(point.id, issuedAt.getTime(), target, 7)],
            [dailyRecord(point.id, issuedAt.getTime(), "2026-06-15", 17)],
          ),
        );
      },
    );

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "test-key", NOW);

    // The hourly collection is untouched: both points stored, status stays "ok".
    // A daily failure must never downgrade the hourly verdict.
    expect(result.status).toBe("ok");
    if (result.status === "ok") {
      expect(result.stored).toBe(WEATHER_POINTS.length);
      expect(result.daily.stored).toBe(1);
      expect(result.daily.failures.map((f) => f.pointId)).toEqual([POINT_A]);
      expect(result.daily.failures[0]?.reason).toContain("daily");
    }

    for (const pointId of [POINT_A, POINT_B]) {
      const hourly = await getWeatherRecordsByRange(
        pool,
        pointId,
        new Date(target - DAY_MS).toISOString(),
        new Date(target + DAY_MS).toISOString(),
      );
      expect(hourly).toHaveLength(1);
    }
    expect(await readDailyRows(pool, POINT_A)).toHaveLength(0);
    expect(await readDailyRows(pool, POINT_B)).toHaveLength(1);
  });

  it("ISOLATION: a point whose HOURLY parse fails still gets its valid daily rows stored", async () => {
    const target = NOW.getTime() + DAY_MS;

    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockImplementation(
      ({ point, issuedAt }) => {
        if (point.id === POINT_A) {
          // Exactly what the boundary returns when the HOURLY schema drifts but
          // the daily block parses: an hourly degrade carrying valid daily rows.
          return Promise.resolve<WeatherFetchResult>({
            ok: false,
            records: [],
            reason: "OpenWeatherMap response failed schema validation",
            daily: {
              ok: true,
              records: [
                dailyRecord(point.id, issuedAt.getTime(), "2026-06-15", 21),
              ],
            },
          });
        }
        return Promise.resolve(
          okResult(
            [record(point.id, issuedAt.getTime(), target, 7)],
            [dailyRecord(point.id, issuedAt.getTime(), "2026-06-15", 17)],
          ),
        );
      },
    );

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "test-key", NOW);

    // The hourly verdict is unchanged: point A degraded, point B stored.
    expect(result.status).toBe("partial");
    if (result.status === "partial") {
      expect(result.stored).toBe(1);
      expect(result.failures.map((f) => f.pointId)).toEqual([POINT_A]);
      // …and the daily rows of BOTH points survived. Dropping point A's daily
      // block here would lose it irreversibly and invisibly: `daily.failures`
      // would stay empty while `daily.stored` silently fell.
      expect(result.daily.stored).toBe(2);
      expect(result.daily.failures).toHaveLength(0);
    }

    const dailyA = await readDailyRows(pool, POINT_A);
    expect(dailyA).toHaveLength(1);
    expect(dailyA[0]?.temp_day).toBe(21);
    expect(await readDailyRows(pool, POINT_B)).toHaveLength(1);
  });

  it("stores daily rows even when EVERY hourly point degrades, and still does not prune", async () => {
    // The retention guard stays hourly: an upstream outage must not delete
    // history. But a daily block that parsed must still be reported and stored.
    const expiredIssuance =
      NOW.getTime() - (WEATHER_RETENTION_DAYS + 10) * DAY_MS;
    await storeWeatherDailyRecords(pool, [
      dailyRecord(POINT_A, expiredIssuance, "2026-06-15", 1),
    ]);

    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockImplementation(
      ({ point, issuedAt }) =>
        Promise.resolve<WeatherFetchResult>({
          ok: false,
          records: [],
          reason: "OpenWeatherMap response failed schema validation",
          daily: {
            ok: true,
            records: [
              dailyRecord(point.id, issuedAt.getTime(), "2026-06-15", 21),
            ],
          },
        }),
    );

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "test-key", NOW);

    expect(result.status).toBe("failed");
    if (result.status === "failed") {
      expect(result.failures).toHaveLength(WEATHER_POINTS.length);
      expect(result.daily.stored).toBe(WEATHER_POINTS.length);
    }

    // Stored, and the expired issuance is still there — no prune on this branch.
    expect(await readDailyRows(pool, POINT_A)).toHaveLength(2);
  });

  it("reports total failure without throwing when every point degrades", async () => {
    const weather = await import("./weather.js");
    vi.spyOn(weather, "fetchWeather").mockResolvedValue({
      ok: false,
      records: [],
      reason: "OpenWeatherMap request timed out",
      daily: {
        ok: false,
        records: [],
        reason: "OpenWeatherMap request timed out",
      },
    });

    const { runWeatherFetchJob } = await import("./weather-job.js");
    const result = await runWeatherFetchJob(pool, "bad-key", NOW);
    expect(result.status).toBe("failed");
    if (result.status === "failed") {
      expect(result.failures).toHaveLength(WEATHER_POINTS.length);
    }
  });
});

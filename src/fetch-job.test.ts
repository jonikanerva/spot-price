import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import type Database from "better-sqlite3";
import { initTestDatabase, closeDatabase } from "./db.js";
import { storePrices } from "./price-store.js";
import { allAreasPresent } from "./fetch-job.js";
import { DELIVERY_AREAS } from "./areas.js";
import type { HourlyPrice } from "./types.js";

/** Generate 96 quarter-hourly price entries (24h) for a given UTC date and area */
const generateDayPrices = (
  utcDateStart: string,
  area: string,
): readonly HourlyPrice[] => {
  const start = new Date(utcDateStart);
  const prices: HourlyPrice[] = [];
  for (let i = 0; i < 96; i++) {
    const deliveryStart = new Date(start.getTime() + i * 15 * 60_000);
    const deliveryEnd = new Date(deliveryStart.getTime() + 15 * 60_000);
    prices.push({
      deliveryStart: deliveryStart.toISOString(),
      deliveryEnd: deliveryEnd.toISOString(),
      priceEurMwh: 30 + Math.random() * 20,
      area,
    });
  }
  return prices;
};

describe("allAreasPresent", () => {
  let db: Database.Database;

  beforeEach(() => {
    db = initTestDatabase();
  });

  afterEach(() => {
    closeDatabase(db);
  });

  it("returns false when no data exists for the date", () => {
    expect(allAreasPresent(db, "2026-03-29")).toBe(false);
  });

  it("returns false when only some areas have data", () => {
    const prices = generateDayPrices("2026-03-29T00:00:00.000Z", "FI");
    storePrices(db, prices);

    expect(allAreasPresent(db, "2026-03-29")).toBe(false);
  });

  it("returns true when all areas have sufficient data", () => {
    for (const area of DELIVERY_AREAS) {
      const prices = generateDayPrices("2026-03-29T00:00:00.000Z", area.code);
      storePrices(db, prices);
    }

    expect(allAreasPresent(db, "2026-03-29")).toBe(true);
  });

  it("returns true with 23 hourly entries (DST spring forward day)", () => {
    // DST day has 23 hours = 92 quarter-hourly entries
    for (const area of DELIVERY_AREAS) {
      const start = new Date("2026-03-29T00:00:00.000Z");
      const prices: HourlyPrice[] = [];
      for (let i = 0; i < 92; i++) {
        const deliveryStart = new Date(start.getTime() + i * 15 * 60_000);
        const deliveryEnd = new Date(deliveryStart.getTime() + 15 * 60_000);
        prices.push({
          deliveryStart: deliveryStart.toISOString(),
          deliveryEnd: deliveryEnd.toISOString(),
          priceEurMwh: 30,
          area: area.code,
        });
      }
      storePrices(db, prices);
    }

    expect(allAreasPresent(db, "2026-03-29")).toBe(true);
  });
});

describe("runFetchJob", () => {
  let db: Database.Database;

  beforeEach(() => {
    db = initTestDatabase();
  });

  afterEach(() => {
    closeDatabase(db);
    vi.restoreAllMocks();
  });

  it("returns tomorrowAvailable: false when API returns empty for tomorrow", async () => {
    const { runFetchJob } = await import("./fetch-job.js");

    // Mock fetchDayAheadPrices to return empty for all dates
    const nordpool = await import("./nordpool.js");
    vi.spyOn(nordpool, "fetchDayAheadPrices").mockResolvedValue([]);

    const result = await runFetchJob(db);

    expect(result.tomorrowAvailable).toBe(false);
  });

  it("returns tomorrowAvailable: true when tomorrow data is stored", async () => {
    const nordpool = await import("./nordpool.js");
    vi.spyOn(nordpool, "fetchDayAheadPrices").mockResolvedValue(
      generateDayPrices("2026-03-30T00:00:00.000Z", "FI") as HourlyPrice[],
    );

    // Pre-fill all areas for today so it skips
    const today = new Date();
    const todayStr = today.toISOString().slice(0, 10);
    for (const area of DELIVERY_AREAS) {
      storePrices(
        db,
        generateDayPrices(`${todayStr}T00:00:00.000Z`, area.code),
      );
    }

    const { runFetchJob } = await import("./fetch-job.js");
    const result = await runFetchJob(db);

    expect(result.tomorrowAvailable).toBe(true);
  });

  it("returns tomorrowAvailable: false when API throws for tomorrow", async () => {
    const nordpool = await import("./nordpool.js");
    let callCount = 0;
    vi.spyOn(nordpool, "fetchDayAheadPrices").mockImplementation(() => {
      callCount++;
      // First call (today) returns empty, second call (tomorrow) throws
      if (callCount >= 2) {
        return Promise.reject(new Error("Network error"));
      }
      return Promise.resolve([]);
    });

    const { runFetchJob } = await import("./fetch-job.js");
    const result = await runFetchJob(db);

    expect(result.tomorrowAvailable).toBe(false);
  });

  it("returns tomorrowAvailable: true when tomorrow data already in DB (skipped)", async () => {
    // Pre-fill all areas for both today and tomorrow
    const today = new Date();
    const todayStr = today.toISOString().slice(0, 10);
    const tomorrow = new Date(today);
    tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
    const tomorrowStr = tomorrow.toISOString().slice(0, 10);

    for (const area of DELIVERY_AREAS) {
      storePrices(
        db,
        generateDayPrices(`${todayStr}T00:00:00.000Z`, area.code),
      );
      storePrices(
        db,
        generateDayPrices(`${tomorrowStr}T00:00:00.000Z`, area.code),
      );
    }

    const { runFetchJob } = await import("./fetch-job.js");
    const result = await runFetchJob(db);

    expect(result.tomorrowAvailable).toBe(true);
    // Both should be skipped
    expect(result.results.every((r) => r.skipped)).toBe(true);
  });
});

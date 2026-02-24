import { describe, it, expect, afterEach } from "vitest";
import { initTestDatabase, closeDatabase } from "./db.js";
import {
  storePrices,
  getPricesByRange,
  countPricesForDate,
} from "./price-store.js";
import type Database from "better-sqlite3";
import type { HourlyPrice } from "./types.js";

describe("price-store", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  const samplePrices: readonly HourlyPrice[] = [
    {
      deliveryStart: "2026-02-24T00:00:00+02:00",
      deliveryEnd: "2026-02-24T01:00:00+02:00",
      priceEurMwh: 30.5,
      area: "FI",
    },
    {
      deliveryStart: "2026-02-24T01:00:00+02:00",
      deliveryEnd: "2026-02-24T02:00:00+02:00",
      priceEurMwh: 28.3,
      area: "FI",
    },
    {
      deliveryStart: "2026-02-24T02:00:00+02:00",
      deliveryEnd: "2026-02-24T03:00:00+02:00",
      priceEurMwh: 25.1,
      area: "FI",
    },
  ];

  it("stores prices and retrieves them", () => {
    db = initTestDatabase();
    const count = storePrices(db, samplePrices);

    expect(count).toBe(3);

    const retrieved = getPricesByRange(
      db,
      "2026-02-24T00:00:00+02:00",
      "2026-02-24T03:00:00+02:00",
      "FI",
    );

    expect(retrieved).toHaveLength(3);
    expect(retrieved[0]?.priceEurMwh).toBe(30.5);
    expect(retrieved[2]?.priceEurMwh).toBe(25.1);
  });

  it("upserts — duplicate inserts update price", () => {
    db = initTestDatabase();
    storePrices(db, samplePrices);

    // Store again with different prices
    const updated: readonly HourlyPrice[] = [
      {
        deliveryStart: "2026-02-24T00:00:00+02:00",
        deliveryEnd: "2026-02-24T01:00:00+02:00",
        priceEurMwh: 99.9,
        area: "FI",
      },
    ];
    storePrices(db, updated);

    const retrieved = getPricesByRange(
      db,
      "2026-02-24T00:00:00+02:00",
      "2026-02-24T01:00:00+02:00",
      "FI",
    );

    expect(retrieved).toHaveLength(1);
    expect(retrieved[0]?.priceEurMwh).toBe(99.9);
  });

  it("counts prices for a date", () => {
    db = initTestDatabase();
    storePrices(db, samplePrices);

    const count = countPricesForDate(db, "2026-02-24", "FI");
    expect(count).toBe(3);

    const countEmpty = countPricesForDate(db, "2026-02-25", "FI");
    expect(countEmpty).toBe(0);
  });
});

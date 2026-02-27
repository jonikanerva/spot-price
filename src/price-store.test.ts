import { describe, it, expect, afterEach } from "vitest";
import { initTestDatabase, closeDatabase } from "./db.js";
import {
  storePrices,
  getPricesByRange,
  countPricesByRange,
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
      deliveryStart: "2026-02-24T00:00:00Z",
      deliveryEnd: "2026-02-24T01:00:00Z",
      priceEurMwh: 30.5,
      area: "FI",
    },
    {
      deliveryStart: "2026-02-24T01:00:00Z",
      deliveryEnd: "2026-02-24T02:00:00Z",
      priceEurMwh: 28.3,
      area: "FI",
    },
    {
      deliveryStart: "2026-02-24T02:00:00Z",
      deliveryEnd: "2026-02-24T03:00:00Z",
      priceEurMwh: 25.1,
      area: "FI",
    },
  ];

  it("stores prices and retrieves them by UTC range", () => {
    db = initTestDatabase();
    const count = storePrices(db, samplePrices);

    expect(count).toBe(3);

    const retrieved = getPricesByRange(
      db,
      "2026-02-24T00:00:00.000Z",
      "2026-02-24T03:00:00.000Z",
      "FI",
    );

    expect(retrieved).toHaveLength(3);
    expect(retrieved[0]?.priceEurMwh).toBe(30.5);
    expect(retrieved[2]?.priceEurMwh).toBe(25.1);
  });

  it("upserts — duplicate inserts update price", () => {
    db = initTestDatabase();
    storePrices(db, samplePrices);

    const updated: readonly HourlyPrice[] = [
      {
        deliveryStart: "2026-02-24T00:00:00Z",
        deliveryEnd: "2026-02-24T01:00:00Z",
        priceEurMwh: 99.9,
        area: "FI",
      },
    ];
    storePrices(db, updated);

    const retrieved = getPricesByRange(
      db,
      "2026-02-24T00:00:00.000Z",
      "2026-02-24T01:00:00.000Z",
      "FI",
    );

    expect(retrieved).toHaveLength(1);
    expect(retrieved[0]?.priceEurMwh).toBe(99.9);
  });

  it("counts prices by UTC range", () => {
    db = initTestDatabase();
    storePrices(db, samplePrices);

    const count = countPricesByRange(
      db,
      "2026-02-24T00:00:00.000Z",
      "2026-02-25T00:00:00.000Z",
      "FI",
    );
    expect(count).toBe(3);

    const countEmpty = countPricesByRange(
      db,
      "2026-02-25T00:00:00.000Z",
      "2026-02-26T00:00:00.000Z",
      "FI",
    );
    expect(countEmpty).toBe(0);
  });

  it("range query is inclusive start, exclusive end", () => {
    db = initTestDatabase();
    storePrices(db, samplePrices);

    // Exactly at boundary: start inclusive
    const withStart = getPricesByRange(
      db,
      "2026-02-24T00:00:00Z",
      "2026-02-24T00:30:00Z",
      "FI",
    );
    expect(withStart).toHaveLength(1);

    // Exactly at boundary: end exclusive
    const withoutEnd = getPricesByRange(
      db,
      "2026-02-24T03:00:00Z",
      "2026-02-24T04:00:00Z",
      "FI",
    );
    expect(withoutEnd).toHaveLength(0);
  });
});

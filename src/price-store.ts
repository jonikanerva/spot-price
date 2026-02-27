import type Database from "better-sqlite3";
import type { HourlyPrice } from "./types.js";

/** Upsert hourly prices into the database (idempotent via INSERT OR REPLACE) */
export const storePrices = (
  db: Database.Database,
  prices: readonly HourlyPrice[],
): number => {
  const insert = db.prepare(`
    INSERT OR REPLACE INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
    VALUES (?, ?, ?, ?)
  `);

  const insertMany = db.transaction((items: readonly HourlyPrice[]): void => {
    for (const p of items) {
      insert.run(p.deliveryStart, p.deliveryEnd, p.priceEurMwh, p.area);
    }
  });

  insertMany(prices);
  return prices.length;
};

interface PriceRow {
  delivery_start: string;
  delivery_end: string;
  price_eur_mwh: number;
  area: string;
}

const rowToHourlyPrice = (r: PriceRow): HourlyPrice => ({
  deliveryStart: r.delivery_start,
  deliveryEnd: r.delivery_end,
  priceEurMwh: r.price_eur_mwh,
  area: r.area,
});

/**
 * Get prices within a UTC time range.
 * All parameters must be UTC ISO 8601 strings (e.g. "2026-02-27T22:00:00.000Z").
 * Returns entries where delivery_start >= startUtc AND delivery_start < endUtc.
 */
export const getPricesByRange = (
  db: Database.Database,
  startUtc: string,
  endUtc: string,
  area: string,
): readonly HourlyPrice[] => {
  const rows = db
    .prepare(
      `SELECT delivery_start, delivery_end, price_eur_mwh, area
       FROM prices
       WHERE area = ? AND delivery_start >= ? AND delivery_start < ?
       ORDER BY delivery_start`,
    )
    .all(area, startUtc, endUtc) as readonly PriceRow[];

  return rows.map(rowToHourlyPrice);
};

/**
 * Count prices within a UTC time range.
 * All parameters must be UTC ISO 8601 strings.
 */
export const countPricesByRange = (
  db: Database.Database,
  startUtc: string,
  endUtc: string,
  area: string,
): number => {
  const result = db
    .prepare(
      `SELECT COUNT(*) as cnt FROM prices
       WHERE area = ? AND delivery_start >= ? AND delivery_start < ?`,
    )
    .get(area, startUtc, endUtc) as { cnt: number };
  return result.cnt;
};

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

/** Get prices for a date range */
export const getPricesByRange = (
  db: Database.Database,
  startInclusive: string,
  endExclusive: string,
  area: string,
): readonly HourlyPrice[] => {
  const rows = db
    .prepare(
      `SELECT delivery_start, delivery_end, price_eur_mwh, area
       FROM prices
       WHERE area = ? AND delivery_start >= ? AND delivery_start < ?
       ORDER BY delivery_start`,
    )
    .all(area, startInclusive, endExclusive) as readonly PriceRow[];

  return rows.map(
    (r): HourlyPrice => ({
      deliveryStart: r.delivery_start,
      deliveryEnd: r.delivery_end,
      priceEurMwh: r.price_eur_mwh,
      area: r.area,
    }),
  );
};

/** Count prices for a given date (YYYY-MM-DD prefix match on delivery_start) */
export const countPricesForDate = (
  db: Database.Database,
  date: string,
  area: string,
): number => {
  const result = db
    .prepare(
      `SELECT COUNT(*) as cnt FROM prices
       WHERE area = ? AND delivery_start LIKE ?`,
    )
    .get(area, `${date}%`) as { cnt: number };
  return result.cnt;
};

/** Get all prices for a specific day (YYYY-MM-DD) */
export const getPricesForDate = (
  db: Database.Database,
  date: string,
  area: string,
): readonly HourlyPrice[] => {
  const rows = db
    .prepare(
      `SELECT delivery_start, delivery_end, price_eur_mwh, area
       FROM prices
       WHERE area = ? AND delivery_start LIKE ?
       ORDER BY delivery_start`,
    )
    .all(area, `${date}%`) as readonly PriceRow[];

  return rows.map(
    (r): HourlyPrice => ({
      deliveryStart: r.delivery_start,
      deliveryEnd: r.delivery_end,
      priceEurMwh: r.price_eur_mwh,
      area: r.area,
    }),
  );
};

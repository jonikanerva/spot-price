import type { Pool } from "pg";
import type { HourlyPrice } from "./types.js";

/** Upsert hourly prices into the database (idempotent via ON CONFLICT) */
export const storePrices = async (
  pool: Pool,
  prices: readonly HourlyPrice[],
): Promise<number> => {
  if (prices.length === 0) {
    return 0;
  }

  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    for (const p of prices) {
      await client.query(
        `INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (delivery_start, area)
         DO UPDATE SET delivery_end = EXCLUDED.delivery_end,
                       price_eur_mwh = EXCLUDED.price_eur_mwh,
                       fetched_at = NOW()`,
        [p.deliveryStart, p.deliveryEnd, p.priceEurMwh, p.area],
      );
    }
    await client.query("COMMIT");
  } catch (err) {
    await client.query("ROLLBACK");
    throw err;
  } finally {
    client.release();
  }

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
export const getPricesByRange = async (
  pool: Pool,
  startUtc: string,
  endUtc: string,
  area: string,
): Promise<readonly HourlyPrice[]> => {
  const { rows } = await pool.query<PriceRow>(
    `SELECT delivery_start, delivery_end, price_eur_mwh, area
     FROM prices
     WHERE area = $1 AND delivery_start >= $2 AND delivery_start < $3
     ORDER BY delivery_start`,
    [area, startUtc, endUtc],
  );

  return rows.map(rowToHourlyPrice);
};

/**
 * Count prices within a UTC time range.
 * All parameters must be UTC ISO 8601 strings.
 */
export const countPricesByRange = async (
  pool: Pool,
  startUtc: string,
  endUtc: string,
  area: string,
): Promise<number> => {
  const { rows } = await pool.query<{ cnt: string }>(
    `SELECT COUNT(*) as cnt FROM prices
     WHERE area = $1 AND delivery_start >= $2 AND delivery_start < $3`,
    [area, startUtc, endUtc],
  );
  return parseInt(rows[0]?.cnt ?? "0", 10);
};

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
 * Fetch prices for several areas at once within a UTC time range, grouped by
 * area. One query instead of N round-trips; used by the forecast to read the
 * neighbour-area (SE1/SE3/EE) price lags it feeds to the model. Areas with no
 * stored rows are simply absent from the returned map — the feature builder
 * neutral-fills them, so a missing neighbour is never an error.
 */
export const getPricesByAreas = async (
  pool: Pool,
  startUtc: string,
  endUtc: string,
  areas: readonly string[],
): Promise<ReadonlyMap<string, readonly HourlyPrice[]>> => {
  const out = new Map<string, HourlyPrice[]>();
  if (areas.length === 0) {
    return out;
  }
  const { rows } = await pool.query<PriceRow>(
    `SELECT delivery_start, delivery_end, price_eur_mwh, area
     FROM prices
     WHERE area = ANY($1) AND delivery_start >= $2 AND delivery_start < $3
     ORDER BY area, delivery_start`,
    [areas, startUtc, endUtc],
  );
  for (const r of rows) {
    const bucket = out.get(r.area) ?? [];
    bucket.push(rowToHourlyPrice(r));
    out.set(r.area, bucket);
  }
  return out;
};

/**
 * Latest published `delivery_start` for an area as a UTC ISO 8601 string, or
 * null when none are stored. Used by the forecast to anchor its series one
 * quarter after the last real price so it never overlaps published data.
 */
export const getLatestDeliveryStart = async (
  pool: Pool,
  area: string,
): Promise<string | null> => {
  const { rows } = await pool.query<{ delivery_start: string }>(
    `SELECT delivery_start FROM prices
     WHERE area = $1
     ORDER BY delivery_start DESC
     LIMIT 1`,
    [area],
  );
  return rows[0]?.delivery_start ?? null;
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

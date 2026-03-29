import { initDatabase, closeDatabase } from "./db.js";
import { randomUUID } from "node:crypto";
import { formatUtcDate } from "./time.js";

/** Number of 15-minute intervals per day */
const INTERVALS_PER_DAY = 96;

/** Generate sample 15-minute prices for a given UTC date (96 intervals) */
const generateDayPrices = (
  dateStr: string,
): readonly { start: string; end: string; price: number }[] =>
  Array.from({ length: INTERVALS_PER_DAY }, (_, i) => {
    const startMs = Date.UTC(
      Number(dateStr.slice(0, 4)),
      Number(dateStr.slice(5, 7)) - 1,
      Number(dateStr.slice(8, 10)),
      Math.floor(i / 4),
      (i % 4) * 15,
    );
    const endMs = startMs + 15 * 60_000;

    const start = new Date(startMs).toISOString();
    const end = new Date(endMs).toISOString();

    // Simulate realistic Finnish spot prices (EUR/MWh)
    // Night hours (22-07) tend to be cheaper
    const hour = Math.floor(i / 4);
    const isNight = hour >= 22 || hour < 7;
    const basePrice = isNight ? 25 : 55;
    const variation = (Math.random() - 0.5) * 30;
    const price = Math.max(0, basePrice + variation);

    return { start, end, price: Math.round(price * 100) / 100 };
  });

const seed = async (): Promise<void> => {
  const pool = await initDatabase();

  try {
    const now = new Date();
    const today = formatUtcDate(now);
    const tomorrow = formatUtcDate(new Date(now.getTime() + 24 * 60 * 60_000));

    const todayPrices = generateDayPrices(today);
    const tomorrowPrices = generateDayPrices(tomorrow);

    const client = await pool.connect();
    try {
      await client.query("BEGIN");
      for (const p of [...todayPrices, ...tomorrowPrices]) {
        await client.query(
          `INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
           VALUES ($1, $2, $3, 'FI')
           ON CONFLICT (delivery_start, area) DO NOTHING`,
          [p.start, p.end, p.price],
        );
      }
      await client.query("COMMIT");
    } catch (err) {
      await client.query("ROLLBACK");
      throw err;
    } finally {
      client.release();
    }

    console.log(
      `Seeded ${String(todayPrices.length)} prices for ${today} and ${String(tomorrowPrices.length)} for ${tomorrow}`,
    );

    // Seed a test user settings
    await pool.query(
      `INSERT INTO user_settings (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh, tax_cents_kwh, vat_percent)
       VALUES ($1, $2, $3, $4, $5, $6)
       ON CONFLICT (user_id) DO NOTHING`,
      ["test-user", 0.45, 3.02, 1.55, 2.79372, 25.5],
    );

    console.log("Seeded test user settings");

    // Seed a test API key
    const rawTestApiKey = "test-api-key-123";
    await pool.query(
      `INSERT INTO api_keys (id, user_id, key_plaintext)
       VALUES ($1, $2, $3)
       ON CONFLICT (user_id) DO NOTHING`,
      [randomUUID(), "test-user", rawTestApiKey],
    );

    console.log("Seeded test API key");
    console.log("Seed complete!");
  } finally {
    await closeDatabase(pool);
  }
};

void seed();

import { initDatabase, closeDatabase } from "./db.js";
import { randomUUID } from "node:crypto";

/** Generate sample hourly prices for a given date (24 hours) */
const generateDayPrices = (
  dateStr: string,
): readonly { start: string; end: string; price: number }[] => {
  const hours = Array.from({ length: 24 }, (_, i) => i);

  return hours.map((hour) => {
    const start = `${dateStr}T${String(hour).padStart(2, "0")}:00:00+02:00`;
    const nextHour = hour + 1;
    const end =
      nextHour < 24
        ? `${dateStr}T${String(nextHour).padStart(2, "0")}:00:00+02:00`
        : `${dateStr}T00:00:00+02:00`; // midnight next day (simplified)

    // Simulate realistic Finnish spot prices (EUR/MWh)
    // Night hours (22-07) tend to be cheaper
    const isNight = hour >= 22 || hour < 7;
    const basePrice = isNight ? 25 : 55;
    const variation = (Math.random() - 0.5) * 30;
    const price = Math.max(0, basePrice + variation);

    return { start, end, price: Math.round(price * 100) / 100 };
  });
};

const seed = (): void => {
  const db = initDatabase();

  try {
    const today = new Date().toISOString().slice(0, 10);
    const tomorrow = new Date(Date.now() + 86400000).toISOString().slice(0, 10);

    const insert = db.prepare(`
      INSERT OR IGNORE INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
      VALUES (?, ?, ?, 'FI')
    `);

    const insertMany = db.transaction(
      (
        prices: readonly { start: string; end: string; price: number }[],
      ): void => {
        for (const p of prices) {
          insert.run(p.start, p.end, p.price);
        }
      },
    );

    const todayPrices = generateDayPrices(today);
    const tomorrowPrices = generateDayPrices(tomorrow);

    insertMany(todayPrices);
    insertMany(tomorrowPrices);

    console.log(
      `Seeded ${String(todayPrices.length)} prices for ${today} and ${String(tomorrowPrices.length)} for ${tomorrow}`,
    );

    // Seed a test user settings
    db.prepare(
      `
      INSERT OR IGNORE INTO user_settings (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh, tax_cents_kwh, vat_percent)
      VALUES (?, ?, ?, ?, ?, ?)
    `,
    ).run("test-user", 0.45, 3.02, 1.55, 2.79372, 25.5);

    console.log("Seeded test user settings");

    // Seed a test API key (hash of "test-api-key-123")
    db.prepare(
      `
      INSERT OR IGNORE INTO api_keys (id, user_id, key_hash, name)
      VALUES (?, ?, ?, ?)
    `,
    ).run(randomUUID(), "test-user", "placeholder-hash", "Dev Test Key");

    console.log("Seeded test API key");
    console.log("Seed complete!");
  } finally {
    closeDatabase(db);
  }
};

seed();

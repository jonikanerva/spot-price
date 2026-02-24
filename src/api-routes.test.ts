import { afterEach, describe, expect, it } from "vitest";
import type Database from "better-sqlite3";
import { createApp } from "./app.js";
import { closeDatabase, initTestDatabase } from "./db.js";
import { hashApiKey } from "./api-keys.js";

const TEST_USER_ID = "user-1";
const TEST_API_KEY = "sp_test_key_123";

const seedUser = (db: Database.Database): void => {
  db.prepare(
    `INSERT OR IGNORE INTO user_settings (
      user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
      tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    TEST_USER_ID,
    0.45,
    3.02,
    1.55,
    2.79372,
    25.5,
    22,
    7,
    "Europe/Helsinki",
  );

  db.prepare(
    `INSERT INTO api_keys (id, user_id, key_hash, name) VALUES (?, ?, ?, ?)`,
  ).run("key-1", TEST_USER_ID, hashApiKey(TEST_API_KEY), "Test key");
};

const seedPrices = (db: Database.Database): void => {
  const now = new Date();
  for (let i = 0; i < 24; i++) {
    const start = new Date(now.getTime() + i * 60 * 60 * 1000);
    const end = new Date(start.getTime() + 60 * 60 * 1000);
    db.prepare(
      `INSERT OR REPLACE INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
       VALUES (?, ?, ?, ?)`,
    ).run(start.toISOString(), end.toISOString(), 30 + i, "FI");
  }
};

describe("API routes", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("returns 401 for price endpoint without API key", async () => {
    db = initTestDatabase();
    const app = createApp(db);

    const response = await app.request("/api/v1/price/now");
    expect(response.status).toBe(401);
  });

  it("creates and lists API keys", async () => {
    db = initTestDatabase();
    const app = createApp(db);

    const createResponse = await app.request("/api/keys", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ userId: TEST_USER_ID, name: "Home Assistant" }),
    });

    expect(createResponse.status).toBe(201);
    const created = (await createResponse.json()) as {
      apiKey: string;
    };
    expect(created.apiKey.startsWith("sp_")).toBe(true);

    const listResponse = await app.request(`/api/keys?userId=${TEST_USER_ID}`);
    expect(listResponse.status).toBe(200);
    const listed = (await listResponse.json()) as {
      keys: readonly { name: string }[];
    };
    expect(listed.keys.length).toBe(1);
    expect(listed.keys[0]?.name).toBe("Home Assistant");
  });

  it("returns current total price with valid API key", async () => {
    db = initTestDatabase();
    seedUser(db);
    seedPrices(db);
    const app = createApp(db);

    const response = await app.request("/api/v1/price/now", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      totalCentsKwh: number;
      spotCentsKwh: number;
    };
    expect(typeof body.totalCentsKwh).toBe("number");
    expect(typeof body.spotCentsKwh).toBe("number");
  });

  it("returns cheapest window with duration", async () => {
    db = initTestDatabase();
    seedUser(db);
    seedPrices(db);
    const app = createApp(db);

    const response = await app.request("/api/v1/price/cheapest?duration=180", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      start: string;
      end: string;
      averageTotalCentsKwh: number;
      prices: readonly unknown[];
    };
    expect(typeof body.start).toBe("string");
    expect(typeof body.end).toBe("string");
    expect(typeof body.averageTotalCentsKwh).toBe("number");
    expect(body.prices.length).toBe(3);
  });

  it("returns 400 for invalid duration", async () => {
    db = initTestDatabase();
    seedUser(db);
    seedPrices(db);
    const app = createApp(db);

    const response = await app.request("/api/v1/price/cheapest?duration=bad", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(400);
  });

  it("gets and updates user settings with valid API key", async () => {
    db = initTestDatabase();
    seedUser(db);
    const app = createApp(db);

    const getResponse = await app.request("/api/v1/settings", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(getResponse.status).toBe(200);

    const updateResponse = await app.request("/api/v1/settings", {
      method: "PUT",
      headers: {
        Authorization: `Bearer ${TEST_API_KEY}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ marginCentsKwh: 0.99, vatPercent: 24 }),
    });
    expect(updateResponse.status).toBe(200);
    const updated = (await updateResponse.json()) as {
      marginCentsKwh: number;
      vatPercent: number;
    };
    expect(updated.marginCentsKwh).toBe(0.99);
    expect(updated.vatPercent).toBe(24);
  });

  it("renders homepage", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const response = await app.request("/");
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html.includes("Spot Price")).toBe(true);
  });
});

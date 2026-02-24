import { afterEach, describe, expect, it } from "vitest";
import type Database from "better-sqlite3";
import { createApp } from "./app.js";
import { closeDatabase, initTestDatabase } from "./db.js";
import { hashApiKey } from "./api-keys.js";

const TEST_USER_ID = "user-1";
const TEST_API_KEY = "sp_test_key_123";

const loginOrSignupAndGetCookie = async (
  app: ReturnType<typeof createApp>,
): Promise<string> => {
  const username = `user_${String(Date.now())}`;
  const response = await app.request("/api/session/login-or-signup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      username,
      password: "password1234",
    }),
  });

  expect(response.status).toBe(200);
  const cookieHeader = response.headers.get("set-cookie");
  expect(cookieHeader).toBeTruthy();
  if (!cookieHeader) {
    throw new Error("Missing session cookie");
  }

  const sessionCookie = cookieHeader.split(";")[0];
  if (!sessionCookie) {
    throw new Error("Invalid session cookie");
  }

  return sessionCookie;
};

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
    const start = new Date(now.getTime() + i * 15 * 60 * 1000);
    const end = new Date(start.getTime() + 15 * 60 * 1000);
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

  it("supports login-or-signup and returns session payload", async () => {
    db = initTestDatabase();
    const app = createApp(db);

    const cookie = await loginOrSignupAndGetCookie(app);
    const sessionResponse = await app.request("/api/session", {
      headers: { Cookie: cookie },
    });

    expect(sessionResponse.status).toBe(200);
    const body = (await sessionResponse.json()) as {
      session: { user: { id: string } } | null;
      username: string | null;
    };
    expect(body.session?.user.id).toBeDefined();
    expect(body.username === null || typeof body.username === "string").toBe(
      true,
    );
  });

  it("creates and lists API keys with session auth", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    const createResponse = await app.request("/api/keys", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Cookie: cookie,
      },
      body: JSON.stringify({ name: "Home Assistant" }),
    });

    expect(createResponse.status).toBe(201);
    const created = (await createResponse.json()) as {
      apiKey: string;
    };
    expect(created.apiKey.startsWith("sp_")).toBe(true);

    const listResponse = await app.request("/api/keys", {
      headers: { Cookie: cookie },
    });
    expect(listResponse.status).toBe(200);
  });

  it("returns 401 for key creation without session", async () => {
    db = initTestDatabase();
    const app = createApp(db);

    const response = await app.request("/api/keys", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "No session key" }),
    });

    expect(response.status).toBe(401);
  });

  it("returns public spot chart data", async () => {
    db = initTestDatabase();
    seedPrices(db);
    const app = createApp(db);

    const response = await app.request("/api/public/spot");
    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      today: readonly unknown[];
      unit: string;
      resolutionMinutes: number;
    };
    expect(body.unit).toBe("c/kWh");
    expect(body.resolutionMinutes).toBe(15);
    expect(Array.isArray(body.today)).toBe(true);
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
      prices: readonly unknown[];
    };
    expect(body.prices.length).toBe(12);
  });

  it("loads and updates me/settings with session auth", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    const getResponse = await app.request("/api/v1/me/settings", {
      headers: { Cookie: cookie },
    });
    expect(getResponse.status).toBe(200);

    const updateResponse = await app.request("/api/v1/me/settings", {
      method: "PUT",
      headers: {
        Cookie: cookie,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ marginCentsKwh: 0.99, vatPercent: 24 }),
    });
    expect(updateResponse.status).toBe(200);
  });

  it("returns me/chart with session auth", async () => {
    db = initTestDatabase();
    seedPrices(db);
    const app = createApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    const response = await app.request("/api/v1/me/chart", {
      headers: { Cookie: cookie },
    });
    expect(response.status).toBe(200);
  });

  it("renders homepage", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const response = await app.request("/");
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html.includes("Login or Signup")).toBe(true);
    expect(html.includes("Spot price (today + tomorrow)")).toBe(true);
  });
});

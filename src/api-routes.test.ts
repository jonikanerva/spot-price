import { afterEach, describe, expect, it } from "vitest";
import type Database from "better-sqlite3";
import { createApp } from "./app.js";
import { closeDatabase, initTestDatabase } from "./db.js";
import { hashApiKey } from "./api-keys.js";
import { formatDateInTimeZone, addDays } from "./time.js";

const TEST_USER_ID = "user-1";
const TEST_API_KEY = "sp_test_key_123";
const HELSINKI_TZ = "Europe/Helsinki";

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
      tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
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
    "FI",
  );

  db.prepare(
    `INSERT INTO api_keys (id, user_id, key_hash, key_plaintext, name) VALUES (?, ?, ?, ?, ?)`,
  ).run(
    "key-1",
    TEST_USER_ID,
    hashApiKey(TEST_API_KEY),
    TEST_API_KEY,
    "default",
  );
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

  it("gets or auto-creates a single API key with session auth", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    // First GET auto-creates a key
    const getResponse = await app.request("/api/keys", {
      headers: { Cookie: cookie },
    });
    expect(getResponse.status).toBe(201);
    const created = (await getResponse.json()) as { apiKey: string };
    expect(created.apiKey.startsWith("sp_")).toBe(true);

    // Second GET returns the same key
    const getResponse2 = await app.request("/api/keys", {
      headers: { Cookie: cookie },
    });
    expect(getResponse2.status).toBe(200);
    const existing = (await getResponse2.json()) as { apiKey: string };
    expect(existing.apiKey).toBe(created.apiKey);
  });

  it("regenerates API key (new key, old invalidated)", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    // Create initial key
    const r1 = await app.request("/api/keys", { headers: { Cookie: cookie } });
    const k1 = (await r1.json()) as { apiKey: string };

    // Regenerate
    const r2 = await app.request("/api/keys/regenerate", {
      method: "POST",
      headers: { Cookie: cookie },
    });
    expect(r2.status).toBe(200);
    const k2 = (await r2.json()) as { apiKey: string };
    expect(k2.apiKey.startsWith("sp_")).toBe(true);
    expect(k2.apiKey).not.toBe(k1.apiKey);

    // GET returns new key
    const r3 = await app.request("/api/keys", { headers: { Cookie: cookie } });
    const k3 = (await r3.json()) as { apiKey: string };
    expect(k3.apiKey).toBe(k2.apiKey);
  });

  it("returns 401 for key access without session", async () => {
    db = initTestDatabase();
    const app = createApp(db);

    const response = await app.request("/api/keys");
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

  it("renders homepage with login and chart elements", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const response = await app.request("/");
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html.includes("Login or Signup")).toBe(true);
    expect(html.includes("Spot price")).toBe(true);
    expect(html.includes("publicChart")).toBe(true);
  });
});

// --- Cheapest endpoint: startTime / endTime filtering ---

/** Seed a single price entry */
const seedPriceEntry = (
  db: Database.Database,
  deliveryStart: string,
  deliveryEnd: string,
  eurMwh: number,
): void => {
  db.prepare(
    `INSERT OR REPLACE INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
     VALUES (?, ?, ?, ?)`,
  ).run(deliveryStart, deliveryEnd, eurMwh, "FI");
};

/** Seed consecutive hourly prices for a Helsinki date string (hours startH..endH-1) */
const seedHourlyRange = (
  db: Database.Database,
  dateStr: string,
  startH: number,
  endH: number,
  eurMwh: number,
  nextDateStr?: string,
): void => {
  for (let h = startH; h < endH; h++) {
    const start = `${dateStr}T${String(h).padStart(2, "0")}:00:00+02:00`;
    const end =
      h + 1 < 24
        ? `${dateStr}T${String(h + 1).padStart(2, "0")}:00:00+02:00`
        : `${nextDateStr ?? dateStr}T00:00:00+02:00`;
    seedPriceEntry(db, start, end, eurMwh);
  }
};

/** Get Helsinki today/tomorrow date strings */
const getHelsinkiDates = (): { today: string; tomorrow: string } => {
  const now = new Date();
  return {
    today: formatDateInTimeZone(now, HELSINKI_TZ),
    tomorrow: formatDateInTimeZone(addDays(now, 1), HELSINKI_TZ),
  };
};

describe("cheapest endpoint — startTime / endTime filtering", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  const setup = (): ReturnType<typeof createApp> => {
    db = initTestDatabase();
    seedUser(db);
    return createApp(db);
  };

  const requestCheapest = async (
    app: ReturnType<typeof createApp>,
    params: string,
  ): Promise<Response> =>
    app.request(`/api/v1/price/cheapest?${params}`, {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

  // --- Requirement 1: duration + startTime ---

  it("startTime only: picks cheapest from tomorrow when tomorrow is cheaper", async () => {
    const app = setup();
    const { today, tomorrow } = getHelsinkiDates();

    // Today 06-22: expensive
    seedHourlyRange(db, today, 6, 22, 100, tomorrow);
    // Tomorrow 00-22: cheap
    seedHourlyRange(db, tomorrow, 0, 22, 10);

    const startTime = `${today}T06:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as { start: string };
    // Cheapest window must be from tomorrow
    expect(body.start).toContain(tomorrow);
  });

  it("startTime only: returns today-only when tomorrow not available", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Only today's prices, no tomorrow
    seedHourlyRange(db, today, 6, 22, 50);

    const startTime = `${today}T06:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as { start: string };
    expect(body.start).toContain(today);
  });

  it("startTime only: window must not start before startTime", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Hour 8-9: very cheap (5 EUR/MWh) — but will be before startTime
    seedHourlyRange(db, today, 8, 10, 5);
    // Hours 10-14: expensive (80 EUR/MWh)
    seedHourlyRange(db, today, 10, 14, 80);
    // Hours 14-16: moderate (40 EUR/MWh) — cheapest AFTER startTime
    seedHourlyRange(db, today, 14, 16, 40);

    const startTime = `${today}T10:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as { start: string; end: string };
    const windowStart = new Date(body.start).getTime();
    const startBound = new Date(startTime).getTime();
    // Window must start at or after startTime
    expect(windowStart).toBeGreaterThanOrEqual(startBound);
    // And specifically should pick the cheaper 14:00 slot, not the 80 EUR/MWh 10:00 slot
    expect(body.start).toContain("T14:");
  });

  // --- Requirement 2: duration + endTime ---

  it("endTime only: window ends at or before endTime", async () => {
    const app = setup();
    const { tomorrow } = getHelsinkiDates();

    // Tomorrow 00-08: expensive (80 EUR/MWh)
    seedHourlyRange(db, tomorrow, 0, 8, 80);
    // Tomorrow 08-12: moderate (30 EUR/MWh) — cheapest before endTime
    seedHourlyRange(db, tomorrow, 8, 12, 30);
    // Tomorrow 12-20: very cheap (5 EUR/MWh) — but after endTime
    seedHourlyRange(db, tomorrow, 12, 20, 5);

    const startTime = `${tomorrow}T00:00:00+02:00`;
    const endTime = `${tomorrow}T12:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}&endTime=${encodeURIComponent(endTime)}`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as { start: string; end: string };
    const windowEnd = new Date(body.end).getTime();
    const endBound = new Date(endTime).getTime();
    // Window must end at or before endTime
    expect(windowEnd).toBeLessThanOrEqual(endBound);
    // Should pick hour 08 (30 EUR), not hour 12+ (5 EUR which is past endTime)
    expect(body.start).toContain("T08:");
  });

  it("endTime only (no startTime): respects endTime bound", async () => {
    const app = setup();
    const { tomorrow } = getHelsinkiDates();

    // Tomorrow 00-10: moderate (40 EUR/MWh) — within endTime
    seedHourlyRange(db, tomorrow, 0, 10, 40);
    // Tomorrow 10-20: cheap (5 EUR/MWh) — past endTime
    seedHourlyRange(db, tomorrow, 10, 20, 5);

    const endTime = `${tomorrow}T10:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&endTime=${encodeURIComponent(endTime)}`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as { start: string; end: string };
    const windowEnd = new Date(body.end).getTime();
    const endBound = new Date(endTime).getTime();
    // Window must not extend past endTime
    expect(windowEnd).toBeLessThanOrEqual(endBound);
  });

  // --- Requirement 3: error when no price data in range ---

  it("returns error when startTime is in a period with no price data", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Seed only today 08-16
    seedHourlyRange(db, today, 8, 16, 50);

    // Ask for a time range far in the future — no data exists
    const startTime = "2027-06-01T00:00:00+03:00";
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}`,
    );

    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("No price data");
  });

  it("returns error when endTime constrains to a period with no data", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Seed today 12-20
    seedHourlyRange(db, today, 12, 20, 50);

    // endTime is before any prices exist
    const startTime = `${today}T06:00:00+02:00`;
    const endTime = `${today}T08:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}&endTime=${encodeURIComponent(endTime)}`,
    );

    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("No price data");
  });
});

describe("OpenAPI spec", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("returns valid OpenAPI 3.1 spec with all expected routes", async () => {
    db = initTestDatabase();
    const app = createApp(db);

    const response = await app.request("/api/v1/openapi.json");
    expect(response.status).toBe(200);

    const spec = (await response.json()) as {
      openapi: string;
      info: { title: string; version: string };
      paths: Record<string, unknown>;
    };

    expect(spec.openapi).toBe("3.1.0");
    expect(spec.info.title).toBe("Spot Price API");
    expect(spec.info.version).toBe("1.0.0");

    // Verify all 8 migrated routes are present
    const paths = Object.keys(spec.paths);
    expect(paths).toContain("/api/v1/price/now");
    expect(paths).toContain("/api/v1/price/today");
    expect(paths).toContain("/api/v1/price/tomorrow");
    expect(paths).toContain("/api/v1/price/cheapest");
    expect(paths).toContain("/api/public/spot");
    expect(paths).toContain("/api/v1/me/settings");
    expect(paths).toContain("/api/v1/me/chart");
  });

  it("returns 400 with error message for invalid cheapest query", async () => {
    db = initTestDatabase();
    seedUser(db);
    seedPrices(db);
    const app = createApp(db);

    // duration=0 is below minimum (1)
    const response = await app.request("/api/v1/price/cheapest?duration=0", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(400);
    const body = (await response.json()) as { error: string };
    expect(body.error).toBeDefined();
    expect(typeof body.error).toBe("string");
  });

  it("returns 400 with error message for invalid settings update", async () => {
    db = initTestDatabase();
    const app = createApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    // vatPercent > 100 is above maximum
    const response = await app.request("/api/v1/me/settings", {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        Cookie: cookie,
      },
      body: JSON.stringify({ vatPercent: 150 }),
    });

    expect(response.status).toBe(400);
    const body = (await response.json()) as { error: string };
    expect(body.error).toBeDefined();
    expect(typeof body.error).toBe("string");
  });
});

import { afterEach, describe, expect, it } from "vitest";
import type Database from "better-sqlite3";
import { closeDatabase, initTestDatabase } from "./db.js";
import { createTestApp } from "./test-utils.js";
import {
  formatDateInTimeZone,
  addDays,
  getUtcRangeForLocalDate,
} from "./time.js";

const TEST_USER_ID = "user-1";
const TEST_API_KEY = "sp_test_key_123";
const HELSINKI_TZ = "Europe/Helsinki";

const loginOrSignupAndGetCookie = async (
  app: ReturnType<typeof createTestApp>,
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
    `INSERT INTO api_keys (id, user_id, key_plaintext) VALUES (?, ?, ?)`,
  ).run("key-1", TEST_USER_ID, TEST_API_KEY);
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
    const app = createTestApp(db);

    const response = await app.request("/api/v1/price/now");
    expect(response.status).toBe(401);
  });

  it("supports login-or-signup and returns session payload", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);

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

  it("returns 400 for malformed JSON in login-or-signup", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);

    const response = await app.request("/api/session/login-or-signup", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{",
    });

    expect(response.status).toBe(400);
    const body = (await response.json()) as { error: string };
    expect(body.error).toBe("Invalid JSON payload");
  });

  it("gets or auto-creates a single API key with session auth", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);
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
    const app = createTestApp(db);
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
    const app = createTestApp(db);

    const response = await app.request("/api/keys");
    expect(response.status).toBe(401);
  });

  it("returns public spot chart data", async () => {
    db = initTestDatabase();
    seedPrices(db);
    const app = createTestApp(db);

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
    const app = createTestApp(db);

    const response = await app.request("/api/v1/price/now", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(200);
  });

  it("returns cheapest window with duration", async () => {
    db = initTestDatabase();
    seedUser(db);
    seedPrices(db);
    const app = createTestApp(db);

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
    const app = createTestApp(db);
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
    const app = createTestApp(db);
    const cookie = await loginOrSignupAndGetCookie(app);

    const response = await app.request("/api/v1/me/chart", {
      headers: { Cookie: cookie },
    });
    expect(response.status).toBe(200);
  });

  it("renders homepage with login and chart elements", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);
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

/**
 * Seed consecutive hourly prices for a Helsinki local date string (hours startH..endH-1).
 * Converts Helsinki local hours to UTC Z format before storing, matching production data.
 * Helsinki winter = UTC+2, so Helsinki hour 14 is stored as UTC hour 12.
 */
const seedHourlyRange = (
  db: Database.Database,
  helsinkiDateStr: string,
  startH: number,
  endH: number,
  eurMwh: number,
): void => {
  // Get the UTC instant for midnight of this Helsinki date
  const { startUtc } = getUtcRangeForLocalDate(helsinkiDateStr, HELSINKI_TZ);
  const midnightUtcMs = new Date(startUtc).getTime();

  for (let h = startH; h < endH; h++) {
    const startMs = midnightUtcMs + h * 60 * 60 * 1000;
    const endMs = startMs + 60 * 60 * 1000;
    seedPriceEntry(
      db,
      new Date(startMs).toISOString(),
      new Date(endMs).toISOString(),
      eurMwh,
    );
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

  const setup = (): ReturnType<typeof createTestApp> => {
    db = initTestDatabase();
    seedUser(db);
    return createTestApp(db);
  };

  const requestCheapest = async (
    app: ReturnType<typeof createTestApp>,
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
    seedHourlyRange(db, today, 6, 22, 100);
    // Tomorrow 00-22: cheap
    seedHourlyRange(db, tomorrow, 0, 22, 10);

    const startTime = `${today}T06:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as { start: string };
    // Cheapest window must be from tomorrow (compare as UTC instants)
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, HELSINKI_TZ);
    const windowStartMs = new Date(body.start).getTime();
    const tomorrowStartMs = new Date(tomorrowUtc.startUtc).getTime();
    const tomorrowEndMs = new Date(tomorrowUtc.endUtc).getTime();
    expect(windowStartMs).toBeGreaterThanOrEqual(tomorrowStartMs);
    expect(windowStartMs).toBeLessThan(tomorrowEndMs);
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
    // Window must fall within today's UTC range
    const todayUtc = getUtcRangeForLocalDate(today, HELSINKI_TZ);
    const windowStartMs = new Date(body.start).getTime();
    expect(windowStartMs).toBeGreaterThanOrEqual(
      new Date(todayUtc.startUtc).getTime(),
    );
    expect(windowStartMs).toBeLessThan(new Date(todayUtc.endUtc).getTime());
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
    // Should pick Helsinki hour 14 (cheapest after startTime).
    // Helsinki hour 14 in winter = UTC hour 12. Verify via UTC instant comparison:
    // the expected UTC start is midnight-UTC-of-Helsinki-date + 14 hours
    const { startUtc } = getUtcRangeForLocalDate(today, HELSINKI_TZ);
    const expectedUtcMs = new Date(startUtc).getTime() + 14 * 60 * 60 * 1000;
    expect(windowStart).toBe(expectedUtcMs);
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
    // Should pick Helsinki hour 08 (30 EUR), not hour 12+ (5 EUR which is past endTime).
    // Verify via UTC instant: Helsinki hour 8 = midnight-UTC + 8h
    const { startUtc } = getUtcRangeForLocalDate(tomorrow, HELSINKI_TZ);
    const expectedUtcMs = new Date(startUtc).getTime() + 8 * 60 * 60 * 1000;
    expect(new Date(body.start).getTime()).toBe(expectedUtcMs);
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

  it("maxPrice: only returns windows where all intervals are <= maxPrice", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Expensive intervals around a cheap 2-hour block
    seedHourlyRange(db, today, 0, 2, 120);
    seedHourlyRange(db, today, 2, 4, 20);
    seedHourlyRange(db, today, 4, 6, 120);

    const startTime = `${today}T00:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=120&startTime=${encodeURIComponent(startTime)}&maxPrice=12`,
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      prices: readonly { totalCentsKwh: number }[];
    };

    expect(body.prices.length).toBe(2);
    expect(body.prices.every((p) => p.totalCentsKwh <= 12)).toBe(true);
  });

  it("maxPrice: does not bridge filtered gaps (contiguity still required)", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Cheap, then expensive (to be filtered out), then cheap again.
    // After maxPrice filtering, remaining 60-min blocks are not contiguous.
    seedHourlyRange(db, today, 0, 1, 20);
    seedHourlyRange(db, today, 1, 2, 120);
    seedHourlyRange(db, today, 2, 3, 20);

    const startTime = `${today}T00:00:00+02:00`;
    const res = await requestCheapest(
      app,
      `duration=120&startTime=${encodeURIComponent(startTime)}&maxPrice=12`,
    );

    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("Not enough contiguous price data");
  });

  it("maxPrice: boundary is inclusive (<=)", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    seedHourlyRange(db, today, 0, 1, 50);
    seedHourlyRange(db, today, 1, 2, 80);

    const startTime = `${today}T00:00:00+02:00`;
    const baselineRes = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}`,
    );
    expect(baselineRes.status).toBe(200);
    const baseline = (await baselineRes.json()) as {
      prices: readonly { totalCentsKwh: number }[];
    };
    const threshold = baseline.prices[0]?.totalCentsKwh;
    expect(threshold).toBeDefined();
    if (threshold === undefined) {
      throw new Error("Missing baseline price for maxPrice boundary test");
    }

    const constrainedRes = await requestCheapest(
      app,
      `duration=60&startTime=${encodeURIComponent(startTime)}&maxPrice=${String(threshold)}`,
    );
    expect(constrainedRes.status).toBe(200);
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

describe("cross-midnight contiguity", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  const setup = (): ReturnType<typeof createTestApp> => {
    db = initTestDatabase();
    seedUser(db);
    return createTestApp(db);
  };

  it("cheapest window spans UTC midnight boundary without gaps", async () => {
    const app = setup();
    const { today, tomorrow } = getHelsinkiDates();

    // Seed today 00-24 (full day): expensive (100 EUR/MWh)
    seedHourlyRange(db, today, 0, 24, 100);
    // Seed tomorrow 00-24 (full day): expensive (100 EUR/MWh)
    seedHourlyRange(db, tomorrow, 0, 24, 100);

    // Now overwrite a cheap 3-hour window that straddles Helsinki midnight
    // Helsinki hours 22, 23 (today) and 00 (tomorrow) = 3 cheap hours
    // These cross the Helsinki day boundary AND the UTC day boundary
    // (Helsinki 22:00 = UTC 20:00, Helsinki 23:00 = UTC 21:00,
    //  Helsinki tomorrow 00:00 = UTC 22:00 — all same UTC date in winter)
    seedHourlyRange(db, today, 22, 24, 5); // today 22-23 Helsinki = cheap
    seedHourlyRange(db, tomorrow, 0, 1, 5); // tomorrow 00-01 Helsinki = cheap

    // Request cheapest 3-hour window — should find the 22:00-01:00 window
    const startTime = `${today}T00:00:00+02:00`;
    const res = await app.request(
      `/api/v1/price/cheapest?duration=180&startTime=${encodeURIComponent(startTime)}`,
      {
        headers: { Authorization: `Bearer ${TEST_API_KEY}` },
      },
    );

    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      start: string;
      end: string;
      prices: readonly { deliveryStart: string; deliveryEnd: string }[];
      averageTotalCentsKwh: number;
    };

    // Should have 3 contiguous hourly entries
    expect(body.prices.length).toBe(3);

    // Verify contiguity: each entry's end equals the next entry's start
    for (let i = 0; i < body.prices.length - 1; i++) {
      const current = body.prices[i];
      const next = body.prices[i + 1];
      if (current && next) {
        expect(new Date(current.deliveryEnd).getTime()).toBe(
          new Date(next.deliveryStart).getTime(),
        );
      }
    }

    // The window should start at Helsinki hour 22 of today
    const { startUtc: todayMidnightUtc } = getUtcRangeForLocalDate(
      today,
      HELSINKI_TZ,
    );
    const expectedStartMs =
      new Date(todayMidnightUtc).getTime() + 22 * 60 * 60 * 1000;
    expect(new Date(body.start).getTime()).toBe(expectedStartMs);

    // The window should end at Helsinki hour 01 of tomorrow
    const { startUtc: tomorrowMidnightUtc } = getUtcRangeForLocalDate(
      tomorrow,
      HELSINKI_TZ,
    );
    const expectedEndMs =
      new Date(tomorrowMidnightUtc).getTime() + 1 * 60 * 60 * 1000;
    expect(new Date(body.end).getTime()).toBe(expectedEndMs);
  });

  it("price/today returns full Helsinki day including pre-UTC-midnight hours", async () => {
    const app = setup();
    const { today } = getHelsinkiDates();

    // Seed full Helsinki day (hours 0-24)
    seedHourlyRange(db, today, 0, 24, 50);

    const res = await app.request("/api/v1/price/today", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      prices: readonly { deliveryStart: string }[];
      available: boolean;
    };

    expect(body.available).toBe(true);
    // Full Helsinki day = 24 hourly entries
    expect(body.prices.length).toBe(24);

    // First entry should be at Helsinki midnight (= UTC 22:00 previous day in winter)
    const { startUtc } = getUtcRangeForLocalDate(today, HELSINKI_TZ);
    const firstPrice = body.prices[0];
    expect(firstPrice).toBeDefined();
    if (firstPrice) {
      expect(new Date(firstPrice.deliveryStart).getTime()).toBe(
        new Date(startUtc).getTime(),
      );
    }
  });
});

describe("OpenAPI spec", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("returns valid OpenAPI 3.1 spec with all expected routes", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);

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
    const app = createTestApp(db);

    // duration=0 is below minimum (1)
    const response = await app.request("/api/v1/price/cheapest?duration=0", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(400);
    const body = (await response.json()) as { error: string };
    expect(body.error).toBeDefined();
    expect(typeof body.error).toBe("string");
  });

  it("returns 400 when maxPrice is blank", async () => {
    db = initTestDatabase();
    seedUser(db);
    seedPrices(db);
    const app = createTestApp(db);

    const response = await app.request(
      "/api/v1/price/cheapest?duration=60&maxPrice=",
      {
        headers: { Authorization: `Bearer ${TEST_API_KEY}` },
      },
    );

    expect(response.status).toBe(400);
    const body = (await response.json()) as { error: string };
    expect(body.error).toContain("maxPrice");
  });

  it("returns 400 with error message for invalid settings update", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);
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

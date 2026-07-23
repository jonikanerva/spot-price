import { afterEach, describe, expect, it } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import { createTestApp } from "./test-utils.js";
import {
  formatDateInTimeZone,
  formatDateTimeInTimeZone,
  addDays,
  getUtcRangeForLocalDate,
} from "./time.js";
import {
  TotalPriceSchema,
  PriceWindowSchema,
  PriceListSchema,
  PriceAllSchema,
  PublicSpotSchema,
  ErrorSchema,
  UserSettingsResponseSchema,
  ChartDataSchema,
} from "./api-schemas.js";

const TEST_USER_ID = "user-1";
const TEST_API_KEY = "sp_test_key_123";
const HELSINKI_TZ = "Europe/Helsinki";

/** Convert Helsinki local date + hour to UTC ms, correctly handling DST */
const helsinkiHourToUtcMs = (date: string, hour: number): number => {
  const { startUtc } = getUtcRangeForLocalDate(date, HELSINKI_TZ);
  const midnightMs = new Date(startUtc).getTime();
  const candidateMs = midnightMs + hour * 3_600_000;

  const actualHour = Number(
    new Intl.DateTimeFormat("en-GB", {
      timeZone: HELSINKI_TZ,
      hour: "2-digit",
      hour12: false,
    }).format(new Date(candidateMs)),
  );

  // Normalize diff to [-12, 12) to handle midnight wrapping
  let diff = actualHour - hour;
  if (diff > 12) diff -= 24;
  if (diff < -12) diff += 24;
  if (diff === 0) return candidateMs;
  return candidateMs - diff * 3_600_000;
};

/** Number of actual hours in a Helsinki local day (23 on spring forward, 25 on fall back) */
const helsinkiDayHours = (date: string): number => {
  const { startUtc } = getUtcRangeForLocalDate(date, HELSINKI_TZ);
  const nextDate = formatDateInTimeZone(
    addDays(new Date(`${date}T12:00:00Z`), 1),
    HELSINKI_TZ,
  );
  const { startUtc: nextMidnight } = getUtcRangeForLocalDate(
    nextDate,
    HELSINKI_TZ,
  );
  return (
    (new Date(nextMidnight).getTime() - new Date(startUtc).getTime()) /
    3_600_000
  );
};

/** Format Helsinki date + hour as ISO 8601 with correct tz offset */
const helsinkiIso = (date: string, hour: number): string =>
  formatDateTimeInTimeZone(
    new Date(helsinkiHourToUtcMs(date, hour)).toISOString(),
    HELSINKI_TZ,
  );

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

const seedUser = async (pool: Pool): Promise<void> => {
  await pool.query(
    `INSERT INTO user_settings (
      user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
      tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
    ON CONFLICT (user_id) DO NOTHING`,
    [
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
    ],
  );

  await pool.query(
    `INSERT INTO api_keys (id, user_id, key_plaintext) VALUES ($1, $2, $3)
    ON CONFLICT (user_id) DO NOTHING`,
    ["key-1", TEST_USER_ID, TEST_API_KEY],
  );
};

const seedPrices = async (pool: Pool): Promise<void> => {
  const now = new Date();
  for (let i = 0; i < 24; i++) {
    const start = new Date(now.getTime() + i * 15 * 60 * 1000);
    const end = new Date(start.getTime() + 15 * 60 * 1000);
    await pool.query(
      `INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
       VALUES ($1, $2, $3, $4)
       ON CONFLICT (delivery_start, area) DO UPDATE SET delivery_end = EXCLUDED.delivery_end, price_eur_mwh = EXCLUDED.price_eur_mwh`,
      [start.toISOString(), end.toISOString(), 30 + i, "FI"],
    );
  }
};

describe("API routes", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("returns 401 for price endpoint without API key", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);

    const response = await app.request("/api/v1/price/now");
    expect(response.status).toBe(401);
  });

  it("supports login-or-signup and returns session payload", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);

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
    pool = await initTestDatabase();
    const app = createTestApp(pool);

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
    pool = await initTestDatabase();
    const app = createTestApp(pool);
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
    pool = await initTestDatabase();
    const app = createTestApp(pool);
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
    pool = await initTestDatabase();
    const app = createTestApp(pool);

    const response = await app.request("/api/keys");
    expect(response.status).toBe(401);
  });

  it("returns public spot chart data", async () => {
    pool = await initTestDatabase();
    await seedPrices(pool);
    const app = createTestApp(pool);

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
    pool = await initTestDatabase();
    await seedUser(pool);
    await seedPrices(pool);
    const app = createTestApp(pool);

    const response = await app.request("/api/v1/price/now", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(200);
  });

  it("returns cheapest window with duration", async () => {
    pool = await initTestDatabase();
    await seedUser(pool);
    await seedPrices(pool);
    const app = createTestApp(pool);

    const response = await app.request("/api/v1/price/cheapest?duration=180", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      prices: readonly unknown[];
      minTotalCentsKwh: number;
      maxTotalCentsKwh: number;
      averageTotalCentsKwh: number;
    };
    expect(body.prices.length).toBe(12);
    expect(typeof body.minTotalCentsKwh).toBe("number");
    expect(typeof body.maxTotalCentsKwh).toBe("number");
    expect(typeof body.averageTotalCentsKwh).toBe("number");
    expect(body.minTotalCentsKwh).toBeLessThanOrEqual(body.maxTotalCentsKwh);
  });

  it("loads and updates me/settings with session auth", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);
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
    pool = await initTestDatabase();
    await seedPrices(pool);
    const app = createTestApp(pool);
    const cookie = await loginOrSignupAndGetCookie(app);

    const response = await app.request("/api/v1/me/chart", {
      headers: { Cookie: cookie },
    });
    expect(response.status).toBe(200);
  });

  it("renders homepage with login and chart elements", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);
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
const seedPriceEntry = async (
  pool: Pool,
  deliveryStart: string,
  deliveryEnd: string,
  eurMwh: number,
): Promise<void> => {
  await pool.query(
    `INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
     VALUES ($1, $2, $3, $4)
     ON CONFLICT (delivery_start, area) DO UPDATE SET delivery_end = EXCLUDED.delivery_end, price_eur_mwh = EXCLUDED.price_eur_mwh`,
    [deliveryStart, deliveryEnd, eurMwh, "FI"],
  );
};

/**
 * Seed consecutive hourly prices for a Helsinki local date string (hours startH..endH-1).
 * Converts Helsinki local hours to UTC using DST-aware conversion before storing.
 */
const seedHourlyRange = async (
  pool: Pool,
  helsinkiDateStr: string,
  startH: number,
  endH: number,
  eurMwh: number,
): Promise<void> => {
  for (let h = startH; h < endH; h++) {
    const startMs = helsinkiHourToUtcMs(helsinkiDateStr, h);
    const endMs = startMs + 3_600_000;
    await seedPriceEntry(
      pool,
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
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  const setup = async (): Promise<ReturnType<typeof createTestApp>> => {
    pool = await initTestDatabase();
    await seedUser(pool);
    return createTestApp(pool);
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
    const app = await setup();
    const { today, tomorrow } = getHelsinkiDates();

    // Today 06-22: expensive
    await seedHourlyRange(pool, today, 6, 22, 100);
    // Tomorrow 00-22: cheap
    await seedHourlyRange(pool, tomorrow, 0, 22, 10);

    const startTime = helsinkiIso(today, 6);
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
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Only today's prices, no tomorrow
    await seedHourlyRange(pool, today, 6, 22, 50);

    const startTime = helsinkiIso(today, 6);
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
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Hour 8-9: very cheap (5 EUR/MWh) — but will be before startTime
    await seedHourlyRange(pool, today, 8, 10, 5);
    // Hours 10-14: expensive (80 EUR/MWh)
    await seedHourlyRange(pool, today, 10, 14, 80);
    // Hours 14-16: moderate (40 EUR/MWh) — cheapest AFTER startTime
    await seedHourlyRange(pool, today, 14, 16, 40);

    const startTime = helsinkiIso(today, 10);
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
    // Should pick Helsinki hour 14 (cheapest after startTime)
    expect(windowStart).toBe(helsinkiHourToUtcMs(today, 14));
  });

  // --- Requirement 2: duration + endTime ---

  it("endTime only: window ends at or before endTime", async () => {
    const app = await setup();
    const { tomorrow } = getHelsinkiDates();

    // Tomorrow 00-08: expensive (80 EUR/MWh)
    await seedHourlyRange(pool, tomorrow, 0, 8, 80);
    // Tomorrow 08-12: moderate (30 EUR/MWh) — cheapest before endTime
    await seedHourlyRange(pool, tomorrow, 8, 12, 30);
    // Tomorrow 12-20: very cheap (5 EUR/MWh) — but after endTime
    await seedHourlyRange(pool, tomorrow, 12, 20, 5);

    const startTime = helsinkiIso(tomorrow, 0);
    const endTime = helsinkiIso(tomorrow, 12);
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
    // Should pick Helsinki hour 08 (30 EUR), not hour 12+ (5 EUR which is past endTime)
    expect(new Date(body.start).getTime()).toBe(
      helsinkiHourToUtcMs(tomorrow, 8),
    );
  });

  it("endTime only (no startTime): respects endTime bound", async () => {
    const app = await setup();
    const { tomorrow } = getHelsinkiDates();

    // Tomorrow 00-10: moderate (40 EUR/MWh) — within endTime
    await seedHourlyRange(pool, tomorrow, 0, 10, 40);
    // Tomorrow 10-20: cheap (5 EUR/MWh) — past endTime
    await seedHourlyRange(pool, tomorrow, 10, 20, 5);

    const endTime = helsinkiIso(tomorrow, 10);
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
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Expensive intervals around a cheap 2-hour block (hours away from DST boundary)
    await seedHourlyRange(pool, today, 8, 10, 120);
    await seedHourlyRange(pool, today, 10, 12, 20);
    await seedHourlyRange(pool, today, 12, 14, 120);

    const startTime = helsinkiIso(today, 8);
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
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Cheap, then expensive (to be filtered out), then cheap again.
    // After maxPrice filtering, remaining 60-min blocks are not contiguous.
    await seedHourlyRange(pool, today, 8, 9, 20);
    await seedHourlyRange(pool, today, 9, 10, 120);
    await seedHourlyRange(pool, today, 10, 11, 20);

    const startTime = helsinkiIso(today, 8);
    const res = await requestCheapest(
      app,
      `duration=120&startTime=${encodeURIComponent(startTime)}&maxPrice=12`,
    );

    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("Not enough contiguous price data");
  });

  it("maxPrice: boundary is inclusive (<=)", async () => {
    const app = await setup();
    const { today } = getHelsinkiDates();

    await seedHourlyRange(pool, today, 0, 1, 50);
    await seedHourlyRange(pool, today, 1, 2, 80);

    const startTime = helsinkiIso(today, 0);
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
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Seed only today 08-16
    await seedHourlyRange(pool, today, 8, 16, 50);

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
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Seed today 12-20
    await seedHourlyRange(pool, today, 12, 20, 50);

    // endTime is before any prices exist
    const startTime = helsinkiIso(today, 6);
    const endTime = helsinkiIso(today, 8);
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
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  const setup = async (): Promise<ReturnType<typeof createTestApp>> => {
    pool = await initTestDatabase();
    await seedUser(pool);
    return createTestApp(pool);
  };

  it("cheapest window spans UTC midnight boundary without gaps", async () => {
    const app = await setup();
    const { today, tomorrow } = getHelsinkiDates();

    // Seed today 00-24 (full day): expensive (100 EUR/MWh)
    await seedHourlyRange(pool, today, 0, 24, 100);
    // Seed tomorrow 00-24 (full day): expensive (100 EUR/MWh)
    await seedHourlyRange(pool, tomorrow, 0, 24, 100);

    // Now overwrite a cheap 3-hour window that straddles Helsinki midnight
    // Helsinki hours 22, 23 (today) and 00 (tomorrow) = 3 cheap hours
    // These cross the Helsinki day boundary AND the UTC day boundary
    // (Helsinki 22:00 = UTC 20:00, Helsinki 23:00 = UTC 21:00,
    //  Helsinki tomorrow 00:00 = UTC 22:00 — all same UTC date in winter)
    await seedHourlyRange(pool, today, 22, 24, 5); // today 22-23 Helsinki = cheap
    await seedHourlyRange(pool, tomorrow, 0, 1, 5); // tomorrow 00-01 Helsinki = cheap

    // Request cheapest 3-hour window — should find the 22:00-01:00 window
    const startTime = helsinkiIso(today, 0);
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
    expect(new Date(body.start).getTime()).toBe(helsinkiHourToUtcMs(today, 22));

    // The window should end at Helsinki hour 01 of tomorrow
    expect(new Date(body.end).getTime()).toBe(helsinkiHourToUtcMs(tomorrow, 1));
  });

  it("price/today returns full Helsinki day including pre-UTC-midnight hours", async () => {
    const app = await setup();
    const { today } = getHelsinkiDates();

    // Seed full Helsinki day (hours 0-24)
    await seedHourlyRange(pool, today, 0, 24, 50);

    const res = await app.request("/api/v1/price/today", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      prices: readonly { deliveryStart: string }[];
      available: boolean;
    };

    expect(body.available).toBe(true);
    // Full Helsinki day: 24 on normal days, 23 on spring forward, 25 on fall back
    expect(body.prices.length).toBe(helsinkiDayHours(today));

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

describe("price/history endpoint", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  const setup = async (): Promise<ReturnType<typeof createTestApp>> => {
    pool = await initTestDatabase();
    await seedUser(pool);
    return createTestApp(pool);
  };

  const requestHistory = async (
    app: ReturnType<typeof createTestApp>,
    params: string,
  ): Promise<Response> =>
    app.request(`/api/v1/price/history?${params}`, {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

  it("returns 401 without an API key", async () => {
    const app = await setup();
    const res = await app.request(
      "/api/v1/price/history?from=2026-04-01&to=2026-04-05",
    );
    expect(res.status).toBe(401);
  });

  it("returns the seeded prices for a valid range (PriceListSchema, available:true)", async () => {
    const app = await setup();
    // Seed three full past Helsinki days.
    await seedHourlyRange(pool, "2026-02-10", 0, 24, 30);
    await seedHourlyRange(pool, "2026-02-11", 0, 24, 40);
    await seedHourlyRange(pool, "2026-02-12", 0, 24, 50);

    const res = await requestHistory(app, "from=2026-02-10&to=2026-02-12");
    expect(res.status).toBe(200);
    const body = (await res.json()) as { available: boolean };
    expect(PriceListSchema.safeParse(body).success).toBe(true);
    expect(body.available).toBe(true);

    const expectedCount =
      helsinkiDayHours("2026-02-10") +
      helsinkiDayHours("2026-02-11") +
      helsinkiDayHours("2026-02-12");
    const typed = body as unknown as { prices: readonly unknown[] };
    expect(typed.prices.length).toBe(expectedCount);
  });

  it("returns 200 available:false with empty prices for an empty valid range", async () => {
    const app = await setup();
    const res = await requestHistory(app, "from=2026-02-10&to=2026-02-12");
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      available: boolean;
      prices: readonly unknown[];
    };
    expect(body.available).toBe(false);
    expect(body.prices).toEqual([]);
  });

  it("returns 400 for invalid date formats", async () => {
    const app = await setup();
    const r1 = await requestHistory(app, "from=2026-13-40&to=2026-04-05");
    expect(r1.status).toBe(400);
    const r2 = await requestHistory(app, "from=not-a-date&to=2026-04-05");
    expect(r2.status).toBe(400);
  });

  it("returns 400 when a datetime is supplied instead of a date", async () => {
    const app = await setup();
    const res = await requestHistory(
      app,
      `from=${encodeURIComponent("2026-04-01T00:00:00Z")}&to=2026-04-05`,
    );
    expect(res.status).toBe(400);
  });

  it("returns 400 when from is after to", async () => {
    const app = await setup();
    const res = await requestHistory(app, "from=2026-04-05&to=2026-04-01");
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("from must be on or before to");
  });

  it("returns 400 when the span exceeds 31 days", async () => {
    const app = await setup();
    const res = await requestHistory(app, "from=2026-01-01&to=2026-03-01");
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("31 days");
  });

  it("accepts exactly 31 inclusive days but rejects 32", async () => {
    const app = await setup();
    // 2026-04-01 .. 2026-05-01 = 31 inclusive days (valid, no data -> available:false)
    const ok = await requestHistory(app, "from=2026-04-01&to=2026-05-01");
    expect(ok.status).toBe(200);
    // 2026-04-01 .. 2026-05-02 = 32 inclusive days (rejected)
    const tooWide = await requestHistory(app, "from=2026-04-01&to=2026-05-02");
    expect(tooWide.status).toBe(400);
  });

  it("returns 404 when user settings are not found", async () => {
    // No seedUser here, so the API key (and thus userId) does not resolve to
    // settings. Provide a key row without settings to reach the handler.
    pool = await initTestDatabase();
    await pool.query(
      `INSERT INTO api_keys (id, user_id, key_plaintext) VALUES ($1, $2, $3)
       ON CONFLICT (user_id) DO NOTHING`,
      ["key-1", TEST_USER_ID, TEST_API_KEY],
    );
    const app = createTestApp(pool);

    const res = await requestHistory(app, "from=2026-04-01&to=2026-04-05");
    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe("User settings not found");
  });

  it("is DST-correct across a spring-forward span", async () => {
    const app = await setup();
    // Helsinki spring-forward is 2026-03-29 (23-hour local day).
    // Seed two days spanning the transition.
    await seedHourlyRange(pool, "2026-03-28", 0, 24, 60);
    await seedHourlyRange(pool, "2026-03-29", 0, 24, 70);

    const res = await requestHistory(app, "from=2026-03-28&to=2026-03-29");
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      available: boolean;
      prices: readonly { deliveryStart: string }[];
    };
    expect(body.available).toBe(true);

    // Count must equal the sum of actual local-day hours (24 + 23 on this span).
    const expectedCount =
      helsinkiDayHours("2026-03-28") + helsinkiDayHours("2026-03-29");
    expect(body.prices.length).toBe(expectedCount);

    // First delivery must start at the from-date's local midnight in UTC.
    const { startUtc } = getUtcRangeForLocalDate("2026-03-28", HELSINKI_TZ);
    const first = body.prices[0];
    expect(first).toBeDefined();
    if (first) {
      expect(new Date(first.deliveryStart).getTime()).toBe(
        new Date(startUtc).getTime(),
      );
    }
  });

  it("returns partial data within a valid range as available:true", async () => {
    const app = await setup();
    // Seed only the middle day of a three-day range.
    await seedHourlyRange(pool, "2026-02-11", 0, 24, 45);

    const res = await requestHistory(app, "from=2026-02-10&to=2026-02-12");
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      available: boolean;
      prices: readonly unknown[];
    };
    expect(body.available).toBe(true);
    expect(body.prices.length).toBe(helsinkiDayHours("2026-02-11"));
  });
});

describe("price/all endpoint", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  const setup = async (): Promise<ReturnType<typeof createTestApp>> => {
    pool = await initTestDatabase();
    await seedUser(pool);
    return createTestApp(pool);
  };

  const requestAll = async (
    app: ReturnType<typeof createTestApp>,
  ): Promise<Response> =>
    app.request("/api/v1/price/all", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });

  interface PriceAllBody {
    today: {
      available: boolean;
      prices: readonly unknown[];
      expectedAt?: string;
    };
    tomorrow: {
      available: boolean;
      prices: readonly unknown[];
      expectedAt?: string;
    };
  }

  it("returns 401 without an API key (wildcard middleware covers /all)", async () => {
    const app = await setup();
    const res = await app.request("/api/v1/price/all");
    expect(res.status).toBe(401);
  });

  it("both days published: today and tomorrow available:true with full days", async () => {
    const app = await setup();
    const { today, tomorrow } = getHelsinkiDates();
    await seedHourlyRange(pool, today, 0, 24, 40);
    await seedHourlyRange(pool, tomorrow, 0, 24, 30);

    const res = await requestAll(app);
    expect(res.status).toBe(200);
    const body = (await res.json()) as PriceAllBody;

    expect(PriceAllSchema.safeParse(body).success).toBe(true);
    expect(body.today.available).toBe(true);
    expect(body.tomorrow.available).toBe(true);
    expect(body.today.prices.length).toBe(helsinkiDayHours(today));
    expect(body.tomorrow.prices.length).toBe(helsinkiDayHours(tomorrow));
  });

  it("tomorrow unpublished: tomorrow available:false with expectedAt, today has no expectedAt", async () => {
    const app = await setup();
    const { today } = getHelsinkiDates();
    // Only today is seeded — tomorrow has not been published yet.
    await seedHourlyRange(pool, today, 0, 24, 50);

    const res = await requestAll(app);
    expect(res.status).toBe(200);
    const body = (await res.json()) as PriceAllBody;

    expect(PriceAllSchema.safeParse(body).success).toBe(true);
    expect(body.today.available).toBe(true);
    expect(body.tomorrow.available).toBe(false);
    expect(body.tomorrow.prices).toEqual([]);
    // Tomorrow carries the publication hint; today never does.
    expect(body.tomorrow.expectedAt).toBe("12:00 UTC");
    expect("expectedAt" in body.today).toBe(false);
  });

  it("nothing published: both days available:false", async () => {
    const app = await setup();
    const res = await requestAll(app);
    expect(res.status).toBe(200);
    const body = (await res.json()) as PriceAllBody;

    expect(PriceAllSchema.safeParse(body).success).toBe(true);
    expect(body.today.available).toBe(false);
    expect(body.tomorrow.available).toBe(false);
    expect(body.today.prices).toEqual([]);
    expect(body.tomorrow.prices).toEqual([]);
    // Empty today never gets an expectedAt hint (that is tomorrow-only).
    expect("expectedAt" in body.today).toBe(false);
    expect(body.tomorrow.expectedAt).toBe("12:00 UTC");
  });

  it("is a published-prices payload only — no forecast or flattened fields", async () => {
    const app = await setup();
    const { today } = getHelsinkiDates();
    await seedHourlyRange(pool, today, 0, 24, 45);

    const res = await requestAll(app);
    expect(res.status).toBe(200);
    const body = (await res.json()) as Record<string, unknown>;

    // No forecast leaks in, and no flattened/parallel top-level shape.
    expect("forecast" in body).toBe(false);
    expect("entries" in body).toBe(false);
    expect("tomorrowAvailable" in body).toBe(false);
    expect("prices" in body).toBe(false);
  });

  it("today's first entry starts at Helsinki midnight (UTC below the edge)", async () => {
    const app = await setup();
    const { today } = getHelsinkiDates();
    await seedHourlyRange(pool, today, 0, 24, 55);

    const res = await requestAll(app);
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      today: { prices: readonly { deliveryStart: string }[] };
    };

    const { startUtc } = getUtcRangeForLocalDate(today, HELSINKI_TZ);
    const first = body.today.prices[0];
    expect(first).toBeDefined();
    if (first) {
      expect(new Date(first.deliveryStart).getTime()).toBe(
        new Date(startUtc).getTime(),
      );
    }
  });

  it("returns 404 when user settings are not found", async () => {
    // Provide an API key row without a settings row so auth passes but the
    // handler cannot resolve contract settings.
    pool = await initTestDatabase();
    await pool.query(
      `INSERT INTO api_keys (id, user_id, key_plaintext) VALUES ($1, $2, $3)
       ON CONFLICT (user_id) DO NOTHING`,
      ["key-1", TEST_USER_ID, TEST_API_KEY],
    );
    const app = createTestApp(pool);

    const res = await requestAll(app);
    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe("User settings not found");
  });
});

describe("OpenAPI spec", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("returns valid OpenAPI 3.1 spec with all expected routes", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);

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

    // Verify all 9 migrated routes are present
    const paths = Object.keys(spec.paths);
    expect(paths).toContain("/api/v1/price/now");
    expect(paths).toContain("/api/v1/price/today");
    expect(paths).toContain("/api/v1/price/tomorrow");
    expect(paths).toContain("/api/v1/price/all");
    expect(paths).toContain("/api/v1/price/cheapest");
    expect(paths).toContain("/api/v1/price/history");
    expect(paths).toContain("/api/public/spot");
    expect(paths).toContain("/api/v1/me/settings");
    expect(paths).toContain("/api/v1/me/chart");
  });

  it("returns 400 with error message for invalid cheapest query", async () => {
    pool = await initTestDatabase();
    await seedUser(pool);
    await seedPrices(pool);
    const app = createTestApp(pool);

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
    pool = await initTestDatabase();
    await seedUser(pool);
    await seedPrices(pool);
    const app = createTestApp(pool);

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
    pool = await initTestDatabase();
    const app = createTestApp(pool);
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

// ---------------------------------------------------------------------------
// Response schema conformance — ensures every endpoint returns data matching
// the declared Zod schemas used for OpenAPI documentation.
// ---------------------------------------------------------------------------

describe("response schema conformance", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  const setup = async (): Promise<ReturnType<typeof createTestApp>> => {
    pool = await initTestDatabase();
    await seedUser(pool);
    await seedPrices(pool);
    return createTestApp(pool);
  };

  it("GET /api/v1/price/now conforms to TotalPriceSchema", async () => {
    const app = await setup();
    const res = await app.request("/api/v1/price/now", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = TotalPriceSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/v1/price/today conforms to PriceListSchema", async () => {
    const app = await setup();
    const res = await app.request("/api/v1/price/today", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = PriceListSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/v1/price/tomorrow conforms to PriceListSchema", async () => {
    const app = await setup();
    const res = await app.request("/api/v1/price/tomorrow", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    // Tomorrow may not be available — both shapes must conform
    const result = PriceListSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/v1/price/cheapest conforms to PriceWindowSchema", async () => {
    const app = await setup();
    const res = await app.request("/api/v1/price/cheapest?duration=60", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = PriceWindowSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/v1/price/history conforms to PriceListSchema", async () => {
    pool = await initTestDatabase();
    await seedUser(pool);
    const app = createTestApp(pool);
    // Seed a fixed past Helsinki day so the response carries real entries.
    await seedHourlyRange(pool, "2026-04-10", 0, 24, 42);

    const res = await app.request(
      "/api/v1/price/history?from=2026-04-10&to=2026-04-10",
      { headers: { Authorization: `Bearer ${TEST_API_KEY}` } },
    );
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = PriceListSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/public/spot conforms to PublicSpotSchema", async () => {
    const app = await setup();
    const res = await app.request("/api/public/spot");
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = PublicSpotSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/v1/me/settings conforms to UserSettingsResponseSchema", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);
    const cookie = await loginOrSignupAndGetCookie(app);

    const res = await app.request("/api/v1/me/settings", {
      headers: { Cookie: cookie },
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = UserSettingsResponseSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("GET /api/v1/me/chart conforms to ChartDataSchema", async () => {
    pool = await initTestDatabase();
    await seedPrices(pool);
    const app = createTestApp(pool);
    const cookie = await loginOrSignupAndGetCookie(app);

    const res = await app.request("/api/v1/me/chart", {
      headers: { Cookie: cookie },
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    const result = ChartDataSchema.safeParse(body);
    expect(result.success).toBe(true);
  });

  it("error responses conform to ErrorSchema", async () => {
    const app = await setup();

    // 401 — missing auth
    const r1 = await app.request("/api/v1/price/now");
    expect(r1.status).toBe(401);
    expect(ErrorSchema.safeParse(await r1.json()).success).toBe(true);

    // 400 — invalid query
    const r2 = await app.request("/api/v1/price/cheapest?duration=0", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(r2.status).toBe(400);
    expect(ErrorSchema.safeParse(await r2.json()).success).toBe(true);
  });
});

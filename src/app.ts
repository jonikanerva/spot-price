import { Hono } from "hono";
import { logger } from "hono/logger";
import type Database from "better-sqlite3";
import {
  calculateTotalPrice,
  calculateTotalPrices,
  findCheapestWindow,
} from "./calculator.js";
import { getCurrentApiKey, regenerateApiKey } from "./api-keys.js";
import {
  apiKeyAuth,
  apiKeyRateLimit,
  globalRateLimit,
  isRegistrationOpen,
  loginRateLimit,
} from "./middleware.js";
import { getPricesForDate } from "./price-store.js";
import { auth } from "./auth.js";
import { eurMwhToCentsKwh } from "./nordpool.js";
import {
  ensureUserSettings,
  getUserSettings,
  upsertUserSettings,
} from "./user-settings.js";
import { addDays, formatDateInTimeZone } from "./time.js";
import type { HourlyPrice } from "./types.js";
import { renderHomePage } from "./ui.js";
import type { AuthSessionUser } from "./session-auth.js";
import { sessionAuth } from "./session-auth.js";
import {
  assignUsername,
  getUserIdByUsername,
  getUsernameByUserId,
  normalizeUsername,
  toInternalEmail,
  validateUsername,
} from "./usernames.js";
import { isValidAreaCode, isValidTimezone } from "./areas.js";

export interface AppEnv {
  Variables: {
    db: Database.Database;
    userId: string;
    sessionUser: AuthSessionUser;
  };
}

const DEFAULT_AREA = "FI";
const HELSINKI_TZ = "Europe/Helsinki";

const parseDuration = (value: string | undefined): number | null => {
  if (!value) {
    return null;
  }
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed <= 0 || parsed > 24 * 60) {
    return null;
  }
  return parsed;
};

const getCurrentAndNextDate = (
  timeZone: string,
): { today: string; tomorrow: string } => {
  const now = new Date();
  return {
    today: formatDateInTimeZone(now, timeZone),
    tomorrow: formatDateInTimeZone(addDays(now, 1), timeZone),
  };
};

const getCurrentPrice = (
  prices: readonly HourlyPrice[],
  now: Date,
): HourlyPrice | null => {
  for (const price of prices) {
    const start = new Date(price.deliveryStart).getTime();
    const end = new Date(price.deliveryEnd).getTime();
    const ts = now.getTime();
    if (ts >= start && ts < end) {
      return price;
    }
  }
  return null;
};

export const createApp = (db: Database.Database): Hono<AppEnv> => {
  const app = new Hono<AppEnv>();

  // Middleware: request logging
  app.use(logger());

  // Middleware: inject database into context
  app.use(async (c, next) => {
    c.set("db", db);
    await next();
  });

  // Global rate limit: 120 req/min per IP (skips /health)
  app.use(globalRateLimit);

  // Health check — verifies DB is accessible
  app.get("/health", (c) => {
    try {
      const dbInstance = c.get("db");
      const result = dbInstance.prepare("SELECT 1 as ok").get() as
        | { ok: number }
        | undefined;

      if (result?.ok === 1) {
        return c.json({ status: "ok", db: "connected" });
      }
      return c.json({ status: "error", db: "query failed" }, 503);
    } catch {
      return c.json({ status: "error", db: "unavailable" }, 503);
    }
  });

  app.get("/", (c) => c.html(renderHomePage()));

  // Login/signup rate limit: 10 req/15min per IP (POST only)
  app.post("/api/session/login-or-signup", loginRateLimit, async (c) => {
    const payload = await c.req.json<{
      username?: string;
      password?: string;
    }>();
    const username = normalizeUsername(payload.username ?? "");
    const password = payload.password;

    if (!validateUsername(username) || !password) {
      return c.json(
        {
          error:
            "username must match [a-z0-9_-] and be 3-32 chars; password is required",
        },
        400,
      );
    }

    const existingUserId = getUserIdByUsername(c.get("db"), username);
    const email = toInternalEmail(username);

    if (existingUserId) {
      const signInResponse = await auth.api.signInEmail({
        headers: c.req.raw.headers,
        body: {
          email,
          password,
          rememberMe: true,
        },
        asResponse: true,
      });
      return signInResponse;
    }

    // User cap: reject signup if at maximum
    if (!isRegistrationOpen(c.get("db"))) {
      return c.json({ error: "Registration is currently closed." }, 403);
    }

    const signUpResponse = await auth.api.signUpEmail({
      headers: c.req.raw.headers,
      body: {
        email,
        password,
        name: username,
      },
      asResponse: true,
    });

    if (signUpResponse.ok) {
      const userRow = c
        .get("db")
        .prepare('SELECT id FROM "user" WHERE email = ?')
        .get(email) as { id: string } | undefined;
      if (userRow?.id) {
        assignUsername(c.get("db"), userRow.id, username);
        ensureUserSettings(c.get("db"), userRow.id);
      }
    }

    return signUpResponse;
  });

  app.post("/api/session/sign-out", async (c) => {
    const response = await auth.api.signOut({
      headers: c.req.raw.headers,
      asResponse: true,
    });
    return response;
  });

  app.get("/api/session", async (c) => {
    const session = await auth.api.getSession({
      headers: c.req.raw.headers,
    });
    const userId =
      (session as { user?: { id?: string } } | null)?.user?.id ?? null;
    const username = userId ? getUsernameByUserId(c.get("db"), userId) : null;
    return c.json({ session, username });
  });

  app.use("/api/keys", sessionAuth);
  app.use("/api/keys/*", sessionAuth);

  /** Get current API key (auto-creates one if none exists) */
  app.get("/api/keys", (c) => {
    const userId = c.get("sessionUser").id;
    ensureUserSettings(c.get("db"), userId);
    const existing = getCurrentApiKey(c.get("db"), userId);
    if (existing) {
      return c.json({ apiKey: existing.key, createdAt: existing.createdAt });
    }
    const created = regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt }, 201);
  });

  /** Regenerate API key (deletes old, creates new) */
  app.post("/api/keys/regenerate", (c) => {
    const userId = c.get("sessionUser").id;
    ensureUserSettings(c.get("db"), userId);
    const created = regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt });
  });

  app.get("/api/public/spot", (c) => {
    const area = c.req.query("area")?.toUpperCase() ?? DEFAULT_AREA;
    if (!isValidAreaCode(area)) {
      return c.json({ error: "Invalid area code" }, 400);
    }

    const { today, tomorrow } = getCurrentAndNextDate(HELSINKI_TZ);
    const todayPrices = getPricesForDate(c.get("db"), today, area).map((p) => ({
      ...p,
      spotCentsKwh: eurMwhToCentsKwh(p.priceEurMwh),
    }));
    const tomorrowPrices = getPricesForDate(c.get("db"), tomorrow, area).map(
      (p) => ({
        ...p,
        spotCentsKwh: eurMwhToCentsKwh(p.priceEurMwh),
      }),
    );

    return c.json({
      area,
      today: todayPrices,
      tomorrow: tomorrowPrices,
      tomorrowAvailable: tomorrowPrices.length > 0,
      unit: "c/kWh",
      resolutionMinutes: 15,
    });
  });

  app.use("/api/v1/price/*", apiKeyAuth, apiKeyRateLimit);
  app.use("/api/v1/me/*", sessionAuth);

  app.get("/api/v1/me/settings", (c) => {
    const userId = c.get("sessionUser").id;
    const settings = ensureUserSettings(c.get("db"), userId);
    return c.json(settings);
  });

  app.put("/api/v1/me/settings", async (c) => {
    const userId = c.get("sessionUser").id;
    const current = ensureUserSettings(c.get("db"), userId);
    const payload = await c.req.json<
      Partial<{
        marginCentsKwh: number;
        transferDayCentsKwh: number;
        transferNightCentsKwh: number;
        taxCentsKwh: number;
        vatPercent: number;
        nightStartHour: number;
        nightEndHour: number;
        timezone: string;
        area: string;
      }>
    >();

    const next = {
      ...current,
      ...payload,
      userId,
    };

    if (next.vatPercent < 0 || next.vatPercent > 100) {
      return c.json({ error: "vatPercent must be 0-100" }, 400);
    }
    if (next.nightStartHour < 0 || next.nightStartHour > 23) {
      return c.json({ error: "nightStartHour must be 0-23" }, 400);
    }
    if (next.nightEndHour < 0 || next.nightEndHour > 23) {
      return c.json({ error: "nightEndHour must be 0-23" }, 400);
    }
    if (!isValidAreaCode(next.area)) {
      return c.json({ error: "Invalid delivery area code" }, 400);
    }
    if (!isValidTimezone(next.timezone)) {
      return c.json({ error: "Invalid timezone" }, 400);
    }

    upsertUserSettings(c.get("db"), next);
    return c.json(next);
  });

  app.get("/api/v1/me/chart", (c) => {
    const userId = c.get("sessionUser").id;
    const settings = ensureUserSettings(c.get("db"), userId);

    const { today, tomorrow } = getCurrentAndNextDate(
      settings.timezone || HELSINKI_TZ,
    );
    const todaySpot = getPricesForDate(c.get("db"), today, settings.area);
    const tomorrowSpot = getPricesForDate(c.get("db"), tomorrow, settings.area);

    return c.json({
      today: calculateTotalPrices(todaySpot, settings),
      tomorrow: calculateTotalPrices(tomorrowSpot, settings),
      tomorrowAvailable: tomorrowSpot.length > 0,
      unit: "c/kWh",
      resolutionMinutes: 15,
    });
  });

  app.get("/api/v1/price/now", (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    const { today, tomorrow } = getCurrentAndNextDate(
      settings.timezone || HELSINKI_TZ,
    );
    const prices = [
      ...getPricesForDate(c.get("db"), today, settings.area),
      ...getPricesForDate(c.get("db"), tomorrow, settings.area),
    ];

    const current = getCurrentPrice(prices, new Date());
    if (!current) {
      return c.json({ error: "No current price available" }, 404);
    }

    const total = calculateTotalPrice(current, settings);
    return c.json(total);
  });

  app.get("/api/v1/price/today", (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    const today = formatDateInTimeZone(
      new Date(),
      settings.timezone || HELSINKI_TZ,
    );
    const prices = getPricesForDate(c.get("db"), today, settings.area);
    if (prices.length === 0) {
      return c.json({ prices: [], available: false });
    }

    return c.json({
      prices: calculateTotalPrices(prices, settings),
      available: true,
    });
  });

  app.get("/api/v1/price/tomorrow", (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    const tomorrow = formatDateInTimeZone(
      addDays(new Date(), 1),
      settings.timezone || HELSINKI_TZ,
    );
    const prices = getPricesForDate(c.get("db"), tomorrow, settings.area);
    if (prices.length === 0) {
      return c.json({ available: false, expectedAt: "14:00 EET", prices: [] });
    }

    return c.json({
      prices: calculateTotalPrices(prices, settings),
      available: true,
    });
  });

  app.get("/api/v1/price/cheapest", (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    const durationMinutes = parseDuration(c.req.query("duration"));
    if (!durationMinutes) {
      return c.json(
        { error: "duration query parameter is required (1-1440 minutes)" },
        400,
      );
    }

    const startTimeParam = c.req.query("startTime");
    const endTimeParam = c.req.query("endTime");
    const startBound = startTimeParam ? new Date(startTimeParam) : null;
    const endBound = endTimeParam ? new Date(endTimeParam) : null;

    if (
      startTimeParam &&
      (!startBound || !Number.isFinite(startBound.getTime()))
    ) {
      return c.json(
        { error: "startTime must be a valid ISO 8601 timestamp" },
        400,
      );
    }
    if (endTimeParam && (!endBound || !Number.isFinite(endBound.getTime()))) {
      return c.json(
        { error: "endTime must be a valid ISO 8601 timestamp" },
        400,
      );
    }

    const now = new Date();
    const { today, tomorrow } = getCurrentAndNextDate(
      settings.timezone || HELSINKI_TZ,
    );
    const prices = [
      ...getPricesForDate(c.get("db"), today, settings.area),
      ...getPricesForDate(c.get("db"), tomorrow, settings.area),
    ];

    const effectiveStart = startBound ?? now;
    const futurePrices = prices.filter((p) => {
      const pStart = new Date(p.deliveryStart).getTime();
      const pEnd = new Date(p.deliveryEnd).getTime();
      if (pStart < effectiveStart.getTime()) {
        return false;
      }
      if (endBound && pEnd > endBound.getTime()) {
        return false;
      }
      return true;
    });

    if (futurePrices.length === 0) {
      return c.json(
        { error: "No price data available for the requested time range" },
        404,
      );
    }

    const totals = calculateTotalPrices(futurePrices, settings);
    const window = findCheapestWindow(totals, durationMinutes);

    if (!window) {
      return c.json(
        {
          error: `Not enough contiguous price data to form a ${String(durationMinutes)}-minute window`,
        },
        404,
      );
    }

    return c.json(window);
  });

  app.get("/api/v1/openapi.json", (c) => {
    return c.json({
      openapi: "3.0.0",
      info: {
        title: "Spot Price API",
        version: "0.1.0",
      },
      paths: {
        "/api/v1/price/now": { get: { summary: "Current total price" } },
        "/api/v1/price/today": {
          get: { summary: "Today's hourly total prices" },
        },
        "/api/v1/price/tomorrow": {
          get: { summary: "Tomorrow's hourly total prices" },
        },
        "/api/v1/price/cheapest": {
          get: { summary: "Cheapest contiguous window" },
        },
      },
    });
  });

  return app;
};

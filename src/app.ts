import { Hono } from "hono";
import { logger } from "hono/logger";
import type Database from "better-sqlite3";
import {
  calculateTotalPrice,
  calculateTotalPrices,
  findCheapestWindow,
} from "./calculator.js";
import { createApiKey, deleteApiKey, listApiKeys } from "./api-keys.js";
import { apiKeyAuth, rateLimit } from "./middleware.js";
import { getPricesForDate } from "./price-store.js";
import { auth } from "./auth.js";
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

export interface AppEnv {
  Variables: {
    db: Database.Database;
    userId: string;
    sessionUser: AuthSessionUser;
  };
}

const AREA = "FI";
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

  app.post("/api/session/sign-up", async (c) => {
    const payload = await c.req.json<{
      email?: string;
      password?: string;
      name?: string;
    }>();
    const email = payload.email?.trim();
    const password = payload.password;

    if (!email || !password) {
      return c.json({ error: "email and password are required" }, 400);
    }

    const name = payload.name?.trim() || email;

    const response = await auth.api.signUpEmail({
      headers: c.req.raw.headers,
      body: {
        email,
        password,
        name,
      },
      asResponse: true,
    });

    return response;
  });

  app.post("/api/session/sign-in", async (c) => {
    const payload = await c.req.json<{
      email?: string;
      password?: string;
      rememberMe?: boolean;
    }>();
    const email = payload.email?.trim();
    const password = payload.password;

    if (!email || !password) {
      return c.json({ error: "email and password are required" }, 400);
    }

    const response = await auth.api.signInEmail({
      headers: c.req.raw.headers,
      body: {
        email,
        password,
        rememberMe: payload.rememberMe ?? true,
      },
      asResponse: true,
    });

    return response;
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
    return c.json({ session });
  });

  app.use("/api/keys", sessionAuth);
  app.use("/api/keys/*", sessionAuth);

  app.post("/api/keys", async (c) => {
    const payload = await c.req.json<{ name?: string }>();
    const userId = c.get("sessionUser").id;
    const name = payload.name?.trim() || "Default";

    ensureUserSettings(c.get("db"), userId);
    const created = createApiKey(c.get("db"), userId, name);

    return c.json(
      {
        apiKey: created.rawKey,
        key: created.keyInfo,
        note: "Store this API key now. It will not be shown again.",
      },
      201,
    );
  });

  app.get("/api/keys", (c) => {
    const userId = c.get("sessionUser").id;
    const keys = listApiKeys(c.get("db"), userId);
    return c.json({ keys });
  });

  app.delete("/api/keys/:id", (c) => {
    const keyId = c.req.param("id");
    const userId = c.get("sessionUser").id;
    const deleted = deleteApiKey(c.get("db"), keyId, userId);
    if (!deleted) {
      return c.json({ error: "API key not found" }, 404);
    }
    return c.json({ deleted: true });
  });

  app.use("/api/v1/price/*", apiKeyAuth, rateLimit);
  app.use("/api/v1/settings", apiKeyAuth, rateLimit);
  app.use("/api/v1/settings/*", apiKeyAuth, rateLimit);

  app.get("/api/v1/settings", (c) => {
    const userId = c.get("userId");
    if (!userId) {
      return c.json({ error: "Unauthorized" }, 401);
    }
    const settings = ensureUserSettings(c.get("db"), userId);
    return c.json(settings);
  });

  app.put("/api/v1/settings", async (c) => {
    const userId = c.get("userId");
    if (!userId) {
      return c.json({ error: "Unauthorized" }, 401);
    }
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

    upsertUserSettings(c.get("db"), next);
    return c.json(next);
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
      ...getPricesForDate(c.get("db"), today, AREA),
      ...getPricesForDate(c.get("db"), tomorrow, AREA),
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
    const prices = getPricesForDate(c.get("db"), today, AREA);
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
    const prices = getPricesForDate(c.get("db"), tomorrow, AREA);
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

    const now = new Date();
    const { today, tomorrow } = getCurrentAndNextDate(
      settings.timezone || HELSINKI_TZ,
    );
    const prices = [
      ...getPricesForDate(c.get("db"), today, AREA),
      ...getPricesForDate(c.get("db"), tomorrow, AREA),
    ];
    const futurePrices = prices.filter(
      (p) => new Date(p.deliveryStart).getTime() >= now.getTime(),
    );

    const totals = calculateTotalPrices(futurePrices, settings);
    const window = findCheapestWindow(totals, durationMinutes);

    if (!window) {
      return c.json(
        { error: "No valid window found for requested duration" },
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

import { OpenAPIHono, createRoute } from "@hono/zod-openapi";
import { Scalar } from "@scalar/hono-api-reference";
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
import { getPricesByRange } from "./price-store.js";
import { auth } from "./auth.js";
import { eurMwhToCentsKwh } from "./nordpool.js";
import {
  ensureUserSettings,
  getUserSettings,
  upsertUserSettings,
} from "./user-settings.js";
import {
  addDays,
  formatDateInTimeZone,
  getUtcRangeForLocalDate,
} from "./time.js";
import type { HourlyPrice, TotalPrice, UserSettings } from "./types.js";
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
import { getDefaultTimezone } from "./areas.js";
import {
  CheapestQuerySchema,
  CheapestWindowSchema,
  ChartDataSchema,
  ErrorSchema,
  PriceListSchema,
  PublicSpotSchema,
  SpotQuerySchema,
  TotalPriceSchema,
  UserSettingsResponseSchema,
  UserSettingsUpdateSchema,
} from "./api-schemas.js";

export interface AppEnv {
  Variables: {
    db: Database.Database;
    userId: string;
    sessionUser: AuthSessionUser;
  };
}

const HELSINKI_TZ = "Europe/Helsinki";

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

// ---------------------------------------------------------------------------
// OpenAPI route definitions
// ---------------------------------------------------------------------------

const publicSpotRoute = createRoute({
  method: "get",
  path: "/api/public/spot",
  tags: ["Public"],
  summary: "Public spot prices (today + tomorrow)",
  description:
    "Returns raw spot prices in c/kWh for the requested delivery area. No authentication required.",
  request: { query: SpotQuerySchema },
  responses: {
    200: {
      content: { "application/json": { schema: PublicSpotSchema } },
      description: "Spot prices for today and tomorrow (if available)",
    },
  },
});

const settingsGetRoute = createRoute({
  method: "get",
  path: "/api/v1/me/settings",
  tags: ["User"],
  summary: "Get user settings",
  description:
    "Returns the authenticated user's electricity contract settings.",
  responses: {
    200: {
      content: { "application/json": { schema: UserSettingsResponseSchema } },
      description: "Current user settings",
    },
  },
});

const settingsPutRoute = createRoute({
  method: "put",
  path: "/api/v1/me/settings",
  tags: ["User"],
  summary: "Update user settings",
  description:
    "Update one or more electricity contract settings. Only provided fields are changed.",
  request: {
    body: {
      content: { "application/json": { schema: UserSettingsUpdateSchema } },
    },
  },
  responses: {
    200: {
      content: { "application/json": { schema: UserSettingsResponseSchema } },
      description: "Updated user settings",
    },
    400: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "Validation error",
    },
  },
});

const chartRoute = createRoute({
  method: "get",
  path: "/api/v1/me/chart",
  tags: ["User"],
  summary: "Chart data (total prices)",
  description:
    "Returns today's and tomorrow's total prices calculated with the user's contract settings.",
  responses: {
    200: {
      content: { "application/json": { schema: ChartDataSchema } },
      description: "Chart data for today and tomorrow",
    },
  },
});

const priceNowRoute = createRoute({
  method: "get",
  path: "/api/v1/price/now",
  tags: ["Price"],
  summary: "Current total price",
  description:
    "Returns the total price breakdown for the current 15-minute delivery interval.",
  security: [{ BearerAuth: [] }],
  responses: {
    200: {
      content: { "application/json": { schema: TotalPriceSchema } },
      description: "Current total price with full breakdown",
    },
    404: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "No current price available or settings not found",
    },
  },
});

const priceTodayRoute = createRoute({
  method: "get",
  path: "/api/v1/price/today",
  tags: ["Price"],
  summary: "Today's hourly total prices",
  description:
    "Returns all total prices for today, calculated with the user's contract settings.",
  security: [{ BearerAuth: [] }],
  responses: {
    200: {
      content: { "application/json": { schema: PriceListSchema } },
      description: "Today's prices",
    },
    404: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "User settings not found",
    },
  },
});

const priceTomorrowRoute = createRoute({
  method: "get",
  path: "/api/v1/price/tomorrow",
  tags: ["Price"],
  summary: "Tomorrow's hourly total prices",
  description:
    "Returns tomorrow's total prices if available. Prices are typically published after 14:00 EET.",
  security: [{ BearerAuth: [] }],
  responses: {
    200: {
      content: { "application/json": { schema: PriceListSchema } },
      description: "Tomorrow's prices (check 'available' field)",
    },
    404: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "User settings not found",
    },
  },
});

const priceCheapestRoute = createRoute({
  method: "get",
  path: "/api/v1/price/cheapest",
  tags: ["Price"],
  summary: "Cheapest contiguous window",
  description:
    "Finds the cheapest contiguous N-minute window from available future prices. Optionally constrain the search with startTime and endTime.",
  security: [{ BearerAuth: [] }],
  request: { query: CheapestQuerySchema },
  responses: {
    200: {
      content: { "application/json": { schema: CheapestWindowSchema } },
      description: "Cheapest window with price breakdown per interval",
    },
    400: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "Invalid query parameters",
    },
    404: {
      content: { "application/json": { schema: ErrorSchema } },
      description:
        "No price data available or insufficient data for requested window",
    },
  },
});

// ---------------------------------------------------------------------------
// App factory
// ---------------------------------------------------------------------------

export const createApp = (db: Database.Database): OpenAPIHono<AppEnv> => {
  const app = new OpenAPIHono<AppEnv>({
    defaultHook: (result, c) => {
      if (!result.success) {
        const messages = result.error.issues
          .map((i) =>
            i.path.length > 0 ? `${i.path.join(".")}: ${i.message}` : i.message,
          )
          .join("; ");
        return c.json({ error: messages }, 400);
      }
    },
  });

  // --- Middleware ----------------------------------------------------------

  app.use(logger());

  app.use(async (c, next) => {
    c.set("db", db);
    await next();
  });

  app.use(globalRateLimit);

  // --- Non-OpenAPI routes (health, HTML, auth, keys) ----------------------

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

    // Check if a user row exists for this email even without a username mapping.
    // This handles accounts created before the usernames table (migration 005)
    // was introduced — their usernames entry is missing but the user row exists.
    const userRowByEmail = c
      .get("db")
      .prepare('SELECT id FROM "user" WHERE email = ?')
      .get(email) as { id: string } | undefined;

    if (existingUserId || userRowByEmail) {
      // Backfill the usernames mapping if it was missing (pre-migration-005 account)
      if (!existingUserId && userRowByEmail) {
        assignUsername(c.get("db"), userRowByEmail.id, username);
      }

      const signInResponse = await auth.api.signInEmail({
        headers: c.req.raw.headers,
        body: { email, password, rememberMe: true },
        asResponse: true,
      });
      return signInResponse;
    }

    if (!isRegistrationOpen(c.get("db"))) {
      return c.json({ error: "Registration is currently closed." }, 403);
    }

    const signUpResponse = await auth.api.signUpEmail({
      headers: c.req.raw.headers,
      body: { email, password, name: username },
      asResponse: true,
    });

    if (signUpResponse.ok) {
      const newUserRow = c
        .get("db")
        .prepare('SELECT id FROM "user" WHERE email = ?')
        .get(email) as { id: string } | undefined;
      if (newUserRow?.id) {
        assignUsername(c.get("db"), newUserRow.id, username);
        ensureUserSettings(c.get("db"), newUserRow.id);
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

  app.post("/api/keys/regenerate", (c) => {
    const userId = c.get("sessionUser").id;
    ensureUserSettings(c.get("db"), userId);
    const created = regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt });
  });

  // --- Auth middleware for API routes --------------------------------------

  app.use("/api/v1/price/*", apiKeyAuth, apiKeyRateLimit);
  app.use("/api/v1/me/*", sessionAuth);

  // --- OpenAPI routes: Public ----------------------------------------------

  app.openapi(publicSpotRoute, (c) => {
    const { area } = c.req.valid("query");

    // Convert local dates to UTC ranges at the boundary
    const tz = getDefaultTimezone(area);
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);

    const todayPrices = getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      todayUtc.endUtc,
      area,
    ).map((p) => ({
      ...p,
      spotCentsKwh: eurMwhToCentsKwh(p.priceEurMwh),
    }));
    const tomorrowPrices = getPricesByRange(
      c.get("db"),
      tomorrowUtc.startUtc,
      tomorrowUtc.endUtc,
      area,
    ).map((p) => ({
      ...p,
      spotCentsKwh: eurMwhToCentsKwh(p.priceEurMwh),
    }));

    return c.json({
      area,
      today: todayPrices,
      tomorrow: tomorrowPrices,
      tomorrowAvailable: tomorrowPrices.length > 0,
      unit: "c/kWh",
      resolutionMinutes: 15,
    });
  });

  // --- OpenAPI routes: User (session-protected) ----------------------------

  app.openapi(settingsGetRoute, (c) => {
    const userId = c.get("sessionUser").id;
    const settings = ensureUserSettings(c.get("db"), userId);
    return c.json(settings);
  });

  app.openapi(settingsPutRoute, (c) => {
    const userId = c.get("sessionUser").id;
    const current = ensureUserSettings(c.get("db"), userId);
    const payload = c.req.valid("json");

    const next: UserSettings = {
      userId,
      marginCentsKwh: payload.marginCentsKwh ?? current.marginCentsKwh,
      transferDayCentsKwh:
        payload.transferDayCentsKwh ?? current.transferDayCentsKwh,
      transferNightCentsKwh:
        payload.transferNightCentsKwh ?? current.transferNightCentsKwh,
      taxCentsKwh: payload.taxCentsKwh ?? current.taxCentsKwh,
      vatPercent: payload.vatPercent ?? current.vatPercent,
      nightStartHour: payload.nightStartHour ?? current.nightStartHour,
      nightEndHour: payload.nightEndHour ?? current.nightEndHour,
      timezone: payload.timezone ?? current.timezone,
      area: payload.area ?? current.area,
    };

    upsertUserSettings(c.get("db"), next);
    return c.json(next, 200);
  });

  app.openapi(chartRoute, (c) => {
    const userId = c.get("sessionUser").id;
    const settings = ensureUserSettings(c.get("db"), userId);

    // Convert local dates to UTC ranges at the boundary
    const tz = settings.timezone || HELSINKI_TZ;
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);

    const todaySpot = getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      todayUtc.endUtc,
      settings.area,
    );
    const tomorrowSpot = getPricesByRange(
      c.get("db"),
      tomorrowUtc.startUtc,
      tomorrowUtc.endUtc,
      settings.area,
    );

    return c.json({
      today: calculateTotalPrices(todaySpot, settings),
      tomorrow: calculateTotalPrices(tomorrowSpot, settings),
      tomorrowAvailable: tomorrowSpot.length > 0,
      unit: "c/kWh",
      resolutionMinutes: 15,
    });
  });

  // --- OpenAPI routes: Price (API key-protected) ---------------------------

  app.openapi(priceNowRoute, (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    // Convert local dates to UTC range spanning today + tomorrow
    const tz = settings.timezone || HELSINKI_TZ;
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);
    const prices = getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      tomorrowUtc.endUtc,
      settings.area,
    );

    const current = getCurrentPrice(prices, new Date());
    if (!current) {
      return c.json({ error: "No current price available" }, 404);
    }

    const total = calculateTotalPrice(current, settings);
    return c.json(total, 200);
  });

  app.openapi(priceTodayRoute, (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    // Convert local "today" to UTC range at the boundary
    const tz = settings.timezone || HELSINKI_TZ;
    const today = formatDateInTimeZone(new Date(), tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const prices = getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      todayUtc.endUtc,
      settings.area,
    );
    if (prices.length === 0) {
      return c.json({ prices: [], available: false }, 200);
    }

    return c.json(
      {
        prices: calculateTotalPrices(prices, settings),
        available: true,
      },
      200,
    );
  });

  app.openapi(priceTomorrowRoute, (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    // Convert local "tomorrow" to UTC range at the boundary
    const tz = settings.timezone || HELSINKI_TZ;
    const tomorrow = formatDateInTimeZone(addDays(new Date(), 1), tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);
    const prices = getPricesByRange(
      c.get("db"),
      tomorrowUtc.startUtc,
      tomorrowUtc.endUtc,
      settings.area,
    );
    if (prices.length === 0) {
      return c.json(
        {
          available: false as const,
          expectedAt: "14:00 EET",
          prices: [] as TotalPrice[],
        },
        200 as const,
      );
    }

    return c.json(
      {
        prices: calculateTotalPrices(prices, settings),
        available: true as const,
      },
      200 as const,
    );
  });

  app.openapi(priceCheapestRoute, (c) => {
    const userId = c.get("userId");
    const settings = getUserSettings(c.get("db"), userId);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const {
      duration: durationMinutes,
      startTime,
      endTime,
    } = c.req.valid("query");
    const startBound = startTime ? new Date(startTime) : null;
    const endBound = endTime ? new Date(endTime) : null;

    const now = new Date();
    const tz = settings.timezone || HELSINKI_TZ;
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    // Use a single UTC range spanning today + tomorrow to ensure contiguous
    // data across midnight (avoids the LIKE prefix gap bug)
    const todayRange = getUtcRangeForLocalDate(today, tz);
    const tomorrowRange = getUtcRangeForLocalDate(tomorrow, tz);
    const prices = getPricesByRange(
      c.get("db"),
      todayRange.startUtc,
      tomorrowRange.endUtc,
      settings.area,
    );

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
        404 as const,
      );
    }

    const totals = calculateTotalPrices(futurePrices, settings);
    const window = findCheapestWindow(totals, durationMinutes);

    if (!window) {
      return c.json(
        {
          error: `Not enough contiguous price data to form a ${String(durationMinutes)}-minute window`,
        },
        404 as const,
      );
    }

    return c.json(window, 200 as const);
  });

  // --- OpenAPI spec + interactive docs ------------------------------------

  app.doc31("/api/v1/openapi.json", {
    openapi: "3.1.0",
    info: {
      title: "Spot Price API",
      version: "1.0.0",
      description:
        "Finnish and Nordic spot electricity price API with total price calculation, cheapest window finder, and per-user contract settings.",
    },
    servers: [{ url: "https://spot.calmdonut.com" }],
    security: [{ BearerAuth: [] }],
    tags: [
      {
        name: "Price",
        description: "Electricity price endpoints (API key required)",
      },
      {
        name: "User",
        description: "User settings and chart data (session required)",
      },
      { name: "Public", description: "Unauthenticated endpoints" },
    ],
  });

  app.openAPIRegistry.registerComponent("securitySchemes", "BearerAuth", {
    type: "http",
    scheme: "bearer",
    description:
      "API key obtained from the web dashboard. Use as: Authorization: Bearer <api-key>",
  });

  app.get(
    "/api/docs",
    Scalar({
      url: "/api/v1/openapi.json",
      theme: "deepSpace",
    }),
  );

  return app;
};

import type { OpenAPIHono } from "@hono/zod-openapi";
import { createRoute } from "@hono/zod-openapi";
import {
  buildPriceWindow,
  calculateTotalPrice,
  calculateTotalPrices,
  findCheapestWindow,
} from "../calculator.js";
import { getPricesByRange } from "../price-store.js";
import { eurMwhToCentsKwh } from "../nordpool.js";
import { getUserSettings } from "../user-settings.js";
import {
  addDays,
  formatDateInTimeZone,
  getCurrentAndNextDate,
  getUtcRangeForLocalDate,
} from "../time.js";
import type { HourlyPrice, TotalPrice, UserSettings } from "../types.js";
import { getDefaultTimezone } from "../areas.js";
import {
  CheapestQuerySchema,
  PriceWindowSchema,
  ErrorSchema,
  PriceListSchema,
  PublicSpotSchema,
  SpotQuerySchema,
  TotalPriceSchema,
} from "../api-schemas.js";
import type { AppEnv } from "../app.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

const getSettings = (c: {
  get: (key: "db" | "userId") => AppEnv["Variables"]["db"] | string;
}): UserSettings | null => {
  const userId = c.get("userId");
  const db = c.get("db");
  if (typeof userId !== "string") {
    return null;
  }
  if (typeof db === "string") {
    return null;
  }
  return getUserSettings(db, userId);
};

// ---------------------------------------------------------------------------
// Route definitions
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
    "Finds the cheapest contiguous N-minute window from available future prices. Optionally constrain the search with startTime, endTime, and maxPrice.",
  security: [{ BearerAuth: [] }],
  request: { query: CheapestQuerySchema },
  responses: {
    200: {
      content: { "application/json": { schema: PriceWindowSchema } },
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
// Registration
// ---------------------------------------------------------------------------

export const registerPriceRoutes = (app: OpenAPIHono<AppEnv>): void => {
  // --- Public spot (no auth) ------------------------------------------------

  app.openapi(publicSpotRoute, (c) => {
    const { area } = c.req.valid("query");

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

  // --- Price endpoints (API key-protected) ----------------------------------

  app.openapi(priceNowRoute, (c) => {
    const settings = getSettings(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    const tz = settings.timezone;
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
    const settings = getSettings(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404);
    }

    const tz = settings.timezone;
    const today = formatDateInTimeZone(new Date(), tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const prices = getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      todayUtc.endUtc,
      settings.area,
    );
    if (prices.length === 0) {
      return c.json(
        {
          start: "",
          end: "",
          startLocal: "",
          endLocal: "",
          minTotalCentsKwh: 0,
          maxTotalCentsKwh: 0,
          averageTotalCentsKwh: 0,
          prices: [],
          available: false,
        },
        200,
      );
    }

    const totals = calculateTotalPrices(prices, settings);
    const window = buildPriceWindow(totals);
    if (!window) {
      return c.json({ error: "Failed to build price window" }, 404);
    }

    return c.json(
      {
        ...window,
        available: true,
      },
      200,
    );
  });

  app.openapi(priceTomorrowRoute, (c) => {
    const settings = getSettings(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const tz = settings.timezone;
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
          start: "",
          end: "",
          startLocal: "",
          endLocal: "",
          minTotalCentsKwh: 0,
          maxTotalCentsKwh: 0,
          averageTotalCentsKwh: 0,
          available: false as const,
          expectedAt: "14:00 EET",
          prices: [] as TotalPrice[],
        },
        200 as const,
      );
    }

    const totals = calculateTotalPrices(prices, settings);
    const window = buildPriceWindow(totals);
    if (!window) {
      return c.json({ error: "Failed to build price window" }, 404 as const);
    }

    return c.json(
      {
        ...window,
        available: true as const,
      },
      200 as const,
    );
  });

  app.openapi(priceCheapestRoute, (c) => {
    const settings = getSettings(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const {
      duration: durationMinutes,
      startTime,
      endTime,
      maxPrice,
    } = c.req.valid("query");
    const startBound = startTime ? new Date(startTime) : null;
    const endBound = endTime ? new Date(endTime) : null;

    const now = new Date();
    const tz = settings.timezone;
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
    const constrainedTotals =
      maxPrice === undefined
        ? totals
        : totals.filter((entry) => entry.totalCentsKwh <= maxPrice);
    const window = findCheapestWindow(constrainedTotals, durationMinutes);

    if (!window) {
      const maxPriceConstraint =
        maxPrice === undefined ? "" : ` at or below ${String(maxPrice)} c/kWh`;
      return c.json(
        {
          error: `Not enough contiguous price data${maxPriceConstraint} to form a ${String(durationMinutes)}-minute window`,
        },
        404 as const,
      );
    }

    return c.json(window, 200 as const);
  });
};

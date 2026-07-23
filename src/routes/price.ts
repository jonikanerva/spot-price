import type { OpenAPIHono } from "@hono/zod-openapi";
import { createRoute } from "@hono/zod-openapi";
import type { Pool } from "pg";
import {
  buildPriceWindow,
  calculateTotalPrice,
  calculateTotalPrices,
  findCheapestWindow,
} from "../calculator.js";
import { getPricesByRange } from "../price-store.js";
import { eurMwhToCentsKwh } from "../nordpool.js";
import { getUserSettingsFromContext } from "./settings-context.js";
import {
  addDays,
  formatDateInTimeZone,
  getCurrentAndNextDate,
  getUtcRangeForLocalDate,
  getUtcRangeForLocalDateSpan,
} from "../time.js";
import type { HourlyPrice, PriceWindow, UserSettings } from "../types.js";
import { getDefaultTimezone } from "../areas.js";
import {
  CheapestQuerySchema,
  PriceWindowSchema,
  ErrorSchema,
  PriceAllSchema,
  PriceHistoryQuerySchema,
  PriceListSchema,
  PublicSpotSchema,
  SpotQuerySchema,
  TotalPriceSchema,
} from "../api-schemas.js";
import type { AppEnv } from "../app.js";

const EMPTY_PRICE_LIST: PriceWindow & { available: false } = {
  start: "",
  end: "",
  startLocal: "",
  endLocal: "",
  minTotalCentsKwh: 0,
  maxTotalCentsKwh: 0,
  averageTotalCentsKwh: 0,
  prices: [],
  available: false,
};

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

const toPublicSpot = (prices: readonly HourlyPrice[]) =>
  prices.map((p) => ({ ...p, spotCentsKwh: eurMwhToCentsKwh(p.priceEurMwh) }));

/**
 * Build one local day's published total-price list for the /all route: read the
 * day's stored prices for the user's area, apply contract terms, and return a
 * PriceList — `available: true` with the window when prices exist, otherwise an
 * empty `available: false` list. `expectedAt` is attached only when the caller
 * passes one (tomorrow), never spread as `undefined`, to satisfy
 * exactOptionalPropertyTypes. Private to /all; /today and /tomorrow are
 * intentionally left untouched. A third consumer of this day-fetch slice should
 * trigger extracting a shared getDayTotalPrices (rule-of-three).
 */
const buildDayPriceList = async (
  db: Pool,
  localDate: string,
  settings: UserSettings,
  expectedAt?: string,
): Promise<PriceWindow & { available: boolean; expectedAt?: string }> => {
  const { startUtc, endUtc } = getUtcRangeForLocalDate(
    localDate,
    settings.timezone,
  );
  const prices = await getPricesByRange(db, startUtc, endUtc, settings.area);
  const totals = calculateTotalPrices(prices, settings);
  const window = buildPriceWindow(totals);
  if (!window) {
    return expectedAt === undefined
      ? EMPTY_PRICE_LIST
      : { ...EMPTY_PRICE_LIST, expectedAt };
  }
  return { ...window, available: true };
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
    "Returns tomorrow's total prices if available. Prices are typically published after 12:00 UTC.",
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

const priceHistoryRoute = createRoute({
  method: "get",
  path: "/api/v1/price/history",
  tags: ["Price"],
  summary: "Historical total prices for a local date range",
  description:
    "Returns all stored total prices for the inclusive local date range " +
    "[from, to]. Totals apply the user's CURRENT contract settings to the " +
    "historical public spot prices — there is no historical settings " +
    "versioning. Dates are YYYY-MM-DD interpreted in the user's timezone; " +
    "the maximum span is 31 days (inclusive). Check the 'available' field: it " +
    "is false with an empty 'prices' array when no data is stored for the range.",
  security: [{ BearerAuth: [] }],
  request: { query: PriceHistoryQuerySchema },
  responses: {
    200: {
      content: { "application/json": { schema: PriceListSchema } },
      description:
        "Historical prices for the range (check 'available'; false + empty when no data stored)",
    },
    400: {
      content: { "application/json": { schema: ErrorSchema } },
      description:
        "Invalid query parameters (bad date, from after to, or span over 31 days)",
    },
    404: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "User settings not found",
    },
  },
});

const priceAllRoute = createRoute({
  method: "get",
  path: "/api/v1/price/all",
  tags: ["Price"],
  summary: "All currently-known hourly total prices (today + tomorrow)",
  description:
    "Returns today's total prices (always) plus tomorrow's (only once Nord Pool " +
    "has published them) in a single payload, so an automation can poll once " +
    "instead of hitting /today and /tomorrow separately. Each day carries its " +
    "own 'available' flag: tomorrow is available:false with an empty 'prices' " +
    "array and an 'expectedAt' hint until it publishes. 'expectedAt' appears " +
    "only on tomorrow. Published prices only — this is not a forecast.",
  security: [{ BearerAuth: [] }],
  responses: {
    200: {
      content: { "application/json": { schema: PriceAllSchema } },
      description:
        "Today's and tomorrow's prices (check each day's 'available' field)",
    },
    404: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "User settings not found",
    },
  },
});

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

export const registerPriceRoutes = (app: OpenAPIHono<AppEnv>): void => {
  // --- Public spot (no auth) ------------------------------------------------

  app.openapi(publicSpotRoute, async (c) => {
    const { area } = c.req.valid("query");

    const tz = getDefaultTimezone(area);
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);

    const todayPrices = toPublicSpot(
      await getPricesByRange(
        c.get("db"),
        todayUtc.startUtc,
        todayUtc.endUtc,
        area,
      ),
    );
    const tomorrowPrices = toPublicSpot(
      await getPricesByRange(
        c.get("db"),
        tomorrowUtc.startUtc,
        tomorrowUtc.endUtc,
        area,
      ),
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

  // --- Price endpoints (API key-protected) ----------------------------------

  app.openapi(priceNowRoute, async (c) => {
    const settings = await getUserSettingsFromContext(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const tz = settings.timezone;
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);
    const prices = await getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      tomorrowUtc.endUtc,
      settings.area,
    );

    const current = getCurrentPrice(prices, new Date());
    if (!current) {
      return c.json({ error: "No current price available" }, 404 as const);
    }

    const total = calculateTotalPrice(current, settings);
    return c.json(total, 200 as const);
  });

  app.openapi(priceTodayRoute, async (c) => {
    const settings = await getUserSettingsFromContext(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const tz = settings.timezone;
    const today = formatDateInTimeZone(new Date(), tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const prices = await getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      todayUtc.endUtc,
      settings.area,
    );
    if (prices.length === 0) {
      return c.json(EMPTY_PRICE_LIST, 200 as const);
    }

    const totals = calculateTotalPrices(prices, settings);
    const window = buildPriceWindow(totals);
    if (!window) {
      return c.json(EMPTY_PRICE_LIST, 200 as const);
    }

    return c.json(
      {
        ...window,
        available: true,
      },
      200 as const,
    );
  });

  app.openapi(priceTomorrowRoute, async (c) => {
    const settings = await getUserSettingsFromContext(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const tz = settings.timezone;
    const tomorrow = formatDateInTimeZone(addDays(new Date(), 1), tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);
    const prices = await getPricesByRange(
      c.get("db"),
      tomorrowUtc.startUtc,
      tomorrowUtc.endUtc,
      settings.area,
    );
    if (prices.length === 0) {
      return c.json(
        { ...EMPTY_PRICE_LIST, expectedAt: "12:00 UTC" },
        200 as const,
      );
    }

    const totals = calculateTotalPrices(prices, settings);
    const window = buildPriceWindow(totals);
    if (!window) {
      return c.json(
        { ...EMPTY_PRICE_LIST, expectedAt: "12:00 UTC" },
        200 as const,
      );
    }

    return c.json(
      {
        ...window,
        available: true as const,
      },
      200 as const,
    );
  });

  app.openapi(priceCheapestRoute, async (c) => {
    const settings = await getUserSettingsFromContext(c);
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
    const prices = await getPricesByRange(
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

  app.openapi(priceHistoryRoute, async (c) => {
    const settings = await getUserSettingsFromContext(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const { from, to } = c.req.valid("query");
    const { startUtc, endUtc } = getUtcRangeForLocalDateSpan(
      from,
      to,
      settings.timezone,
    );
    const prices = await getPricesByRange(
      c.get("db"),
      startUtc,
      endUtc,
      settings.area,
    );
    // An empty-but-valid range is a real state, not an error: /history is a
    // list endpoint like /today, so respond 200 with available:false (404 is
    // reserved for settings-not-found). Matches VISION's "not yet, never a guess".
    if (prices.length === 0) {
      return c.json(EMPTY_PRICE_LIST, 200 as const);
    }

    const totals = calculateTotalPrices(prices, settings);
    const window = buildPriceWindow(totals);
    if (!window) {
      return c.json(EMPTY_PRICE_LIST, 200 as const);
    }

    return c.json({ ...window, available: true as const }, 200 as const);
  });

  app.openapi(priceAllRoute, async (c) => {
    const settings = await getUserSettingsFromContext(c);
    if (!settings) {
      return c.json({ error: "User settings not found" }, 404 as const);
    }

    const { today, tomorrow } = getCurrentAndNextDate(settings.timezone);
    const db = c.get("db");
    const [todayList, tomorrowList] = await Promise.all([
      buildDayPriceList(db, today, settings),
      buildDayPriceList(db, tomorrow, settings, "12:00 UTC"),
    ]);

    return c.json({ today: todayList, tomorrow: tomorrowList }, 200 as const);
  });
};

import type { OpenAPIHono } from "@hono/zod-openapi";
import { createRoute } from "@hono/zod-openapi";
import { applyContractTerms, extractHourInTimeZone } from "../calculator.js";
import { eurMwhToCentsKwh } from "../nordpool.js";
import { getLatestDeliveryStart, getPricesByRange } from "../price-store.js";
import { getFingridRecordsByRange } from "../fingrid-store.js";
import { getUserSettings } from "../user-settings.js";
import { formatDateTimeInTimeZone } from "../time.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../fingrid.js";
import {
  buildForecast,
  FLOOR_HISTORY_DAYS,
  FORECAST_DAYS,
  priceFloorFromHistory,
  quarterKey,
} from "../forecast.js";
import type { ForecastEntry, ForecastResult, UserSettings } from "../types.js";
import { ForecastQuerySchema, ForecastResponseSchema } from "../api-schemas.js";
import type { AppEnv } from "../app.js";

const QUARTER_MS = 15 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;
const FORECAST_AREA = "FI";

const getSettings = async (c: {
  get: (key: "db" | "userId") => AppEnv["Variables"]["db"] | string;
}): Promise<UserSettings | null> => {
  const userId = c.get("userId");
  const db = c.get("db");
  if (typeof userId !== "string" || typeof db === "string") {
    return null;
  }
  return await getUserSettings(db, userId);
};

/** Build a quarter-keyed map of stored FI spot prices in c/kWh. */
const spotPricesByKey = (
  prices: readonly { deliveryStart: string; priceEurMwh: number }[],
): Map<string, number> => {
  const out = new Map<string, number>();
  for (const p of prices) {
    out.set(
      quarterKey(new Date(p.deliveryStart).getTime()),
      eurMwhToCentsKwh(p.priceEurMwh),
    );
  }
  return out;
};

/** Apply the user's contract terms to each predicted spot quarter. */
const toForecastEntries = (
  result: ForecastResult,
  settings: UserSettings,
): ForecastEntry[] =>
  result.series.map((point) => {
    const startMs = new Date(point.start).getTime();
    const endIso = new Date(startMs + QUARTER_MS).toISOString();
    const hour = extractHourInTimeZone(point.start, settings.timezone);
    // Single source of total-price truth, shared with the real-price path.
    const breakdown = applyContractTerms(
      point.estimatedSpotCentsKwh,
      settings,
      hour,
    );
    return {
      start: point.start,
      end: endIso,
      localStart: formatDateTimeInTimeZone(point.start, settings.timezone),
      localEnd: formatDateTimeInTimeZone(endIso, settings.timezone),
      estimatedSpotCentsKwh: breakdown.spotCentsKwh,
      estimatedTotalCentsKwh: breakdown.totalCentsKwh,
      estimated: true,
    };
  });

const forecastRoute = createRoute({
  method: "get",
  path: "/api/v1/price/forecast",
  tags: ["Forecast"],
  summary: "FI price forecast (estimate)",
  description:
    "Returns a clearly-labelled price ESTIMATE for the days Nord Pool has not " +
    "published yet, derived from public Fingrid grid data plus stored price " +
    "history using simple closed-form math (no ML). Structurally distinct from " +
    "the real-price endpoints — every money field is named `estimated*` and " +
    "carries `estimated: true`, the response carries `forecast: true`. FI only; " +
    "other areas return available:false. When data is insufficient the response " +
    "is degraded/low-confidence rather than a confident guess.",
  security: [{ BearerAuth: [] }],
  request: { query: ForecastQuerySchema },
  responses: {
    200: {
      content: { "application/json": { schema: ForecastResponseSchema } },
      description:
        "Forecast estimate (check available/degraded/confidence). Non-FI areas " +
        "return available:false.",
    },
    404: {
      content: {
        "application/json": {
          schema: ForecastResponseSchema,
        },
      },
      description: "User settings not found",
    },
  },
});

export const registerForecastRoutes = (app: OpenAPIHono<AppEnv>): void => {
  app.openapi(forecastRoute, async (c) => {
    const { area } = c.req.valid("query");
    const generatedAt = new Date().toISOString();

    const settings = await getSettings(c);
    if (!settings) {
      return c.json(
        {
          forecast: true as const,
          area,
          available: false,
          degraded: true,
          confidence: "low" as const,
          reason: "User settings not found",
          generatedAt,
          unit: "c/kWh",
          resolutionMinutes: 15,
          entries: [],
        },
        404 as const,
      );
    }

    // The forecast is FI-only: Fingrid is the Finnish grid operator. Any other
    // area is a permanent "unavailable here", not an error (VISION → The forecast).
    if (area !== FORECAST_AREA) {
      return c.json(
        {
          forecast: true as const,
          area,
          available: false,
          degraded: true,
          confidence: "low" as const,
          reason: "Forecast available for FI only",
          generatedAt,
          unit: "c/kWh",
          resolutionMinutes: 15,
          entries: [],
        },
        200 as const,
      );
    }

    const db = c.get("db");

    // Anchor the series one quarter after the last published FI price so it
    // never overlaps real prices.
    const lastPublished = await getLatestDeliveryStart(db, FORECAST_AREA);
    const nowMs = new Date(generatedAt).getTime();
    const seriesStartMs = lastPublished
      ? new Date(lastPublished).getTime() + QUARTER_MS
      : nowMs;
    const seriesEndMs = seriesStartMs + FORECAST_DAYS * DAY_MS;

    // Read context off the request path only: ~30d of FI spot history (for the
    // fit + floor) through the last published quarter, and the four Fingrid
    // series for the full [history, seriesEnd) window.
    const historyStartMs = seriesStartMs - FLOOR_HISTORY_DAYS * DAY_MS;
    const historyStartUtc = new Date(historyStartMs).toISOString();
    const fingridEndUtc = new Date(seriesEndMs).toISOString();

    const prices = await getPricesByRange(
      db,
      historyStartUtc,
      new Date(seriesStartMs).toISOString(),
      FORECAST_AREA,
    );
    const spot = spotPricesByKey(prices);

    const [windForecast, windActual, consumptionForecast, consumptionActual] =
      await Promise.all([
        getFingridRecordsByRange(
          db,
          DATASET_WIND_FORECAST,
          historyStartUtc,
          fingridEndUtc,
        ),
        getFingridRecordsByRange(
          db,
          DATASET_WIND_ACTUAL,
          historyStartUtc,
          fingridEndUtc,
        ),
        getFingridRecordsByRange(
          db,
          DATASET_CONSUMPTION_FORECAST,
          historyStartUtc,
          fingridEndUtc,
        ),
        getFingridRecordsByRange(
          db,
          DATASET_CONSUMPTION_ACTUAL,
          historyStartUtc,
          fingridEndUtc,
        ),
      ]);

    // No Fingrid data at all means the cron has not populated the table (e.g.
    // no key configured, or upstream down). Degrade rather than guess.
    const hasFingridData =
      windForecast.length +
        windActual.length +
        consumptionForecast.length +
        consumptionActual.length >
      0;
    if (!hasFingridData) {
      return c.json(
        {
          forecast: true as const,
          area,
          available: false,
          degraded: true,
          confidence: "low" as const,
          reason: "No Fingrid grid data available for the forecast window",
          generatedAt,
          unit: "c/kWh",
          resolutionMinutes: 15,
          entries: [],
        },
        200 as const,
      );
    }

    const floor = priceFloorFromHistory(spot);
    const result = buildForecast(
      {
        spotPricesByKey: spot,
        windForecast,
        windActual,
        consumptionForecast,
        consumptionActual,
        seriesStartMs,
        seriesEndMs,
      },
      { floor },
    );

    const entries = toForecastEntries(result, settings);

    // Degraded/low-confidence iff the fit fell back to defaults or a hard
    // outage forced zero-seeding. Tail extension alone is NOT degraded.
    const degraded =
      result.diagnostics.fitUsedDefault ||
      result.diagnostics.zeroSeededQuarters > 0;

    return c.json(
      {
        forecast: true as const,
        area,
        available: true,
        degraded,
        confidence: degraded ? ("low" as const) : ("normal" as const),
        ...(degraded
          ? {
              reason: result.diagnostics.fitUsedDefault
                ? "Insufficient overlap to fit; using default coefficients"
                : "Some quarters had no input data and were zero-seeded",
            }
          : {}),
        generatedAt,
        unit: "c/kWh",
        resolutionMinutes: 15,
        entries,
      },
      200 as const,
    );
  });
};

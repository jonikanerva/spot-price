import type { OpenAPIHono } from "@hono/zod-openapi";
import { createRoute } from "@hono/zod-openapi";
import { applyContractTerms, extractHourInTimeZone } from "../calculator.js";
import { eurMwhToCentsKwh } from "../nordpool.js";
import {
  getLatestDeliveryStart,
  getPricesByAreas,
  getPricesByRange,
} from "../price-store.js";
import { getFingridRecordsByRange } from "../fingrid-store.js";
import { getUserSettingsFromContext } from "./settings-context.js";
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
  quarterKey,
  sanityBoundFromHistory,
} from "../forecast.js";
import type { ForecastEntry, ForecastResult, UserSettings } from "../types.js";
import { ForecastQuerySchema, ForecastResponseSchema } from "../api-schemas.js";
import { CALIBRATED_BANDS } from "../conformal-artifact.js";
import type { AppEnv } from "../app.js";

/**
 * The top-level band descriptor surfaced on the success response: the method,
 * the nominal/observed coverage, whether bands shipped, and when the artifact
 * was generated. Always present so a consumer can tell why bound fields are
 * absent (currently dark: `calibrated: false`). Orthogonal to
 * `degraded`/`confidence`.
 */
const bandsDescriptor = (): {
  method: "empirical-residual";
  nominalCoverage: number;
  observedCoverage: number | null;
  calibrated: boolean;
  generatedAt: string;
} => ({
  method: CALIBRATED_BANDS.method,
  nominalCoverage: CALIBRATED_BANDS.nominalCoverage,
  observedCoverage: CALIBRATED_BANDS.observedCoverage,
  calibrated: CALIBRATED_BANDS.calibrated,
  generatedAt: CALIBRATED_BANDS.generatedAt,
});

const QUARTER_MS = 15 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;
const FORECAST_AREA = "FI";
/**
 * Neighbour Nord Pool areas whose lagged prices inform the FI estimate (their
 * day-ahead prices are already stored for the FI fetch's sibling areas). A
 * missing neighbour is neutral-filled by the feature builder, never an error.
 */
const NEIGHBOR_AREAS: readonly string[] = ["SE1", "SE3", "EE"];

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
    const entry: ForecastEntry = {
      start: point.start,
      end: endIso,
      localStart: formatDateTimeInTimeZone(point.start, settings.timezone),
      localEnd: formatDateTimeInTimeZone(endIso, settings.timezone),
      estimatedSpotCentsKwh: breakdown.spotCentsKwh,
      estimatedTotalCentsKwh: breakdown.totalCentsKwh,
      estimated: true,
    };
    // Band bounds are present only when a calibrated artifact shipped AND this
    // quarter carried a real prediction. `applyContractTerms` is a monotone
    // affine transform in spot, so applying it to each bound independently
    // preserves the low ≤ point ≤ high ordering.
    const { estimatedSpotLowCentsKwh: low, estimatedSpotHighCentsKwh: high } =
      point;
    if (low !== undefined && high !== undefined) {
      return {
        ...entry,
        estimatedSpotLowCentsKwh: low,
        estimatedSpotHighCentsKwh: high,
        estimatedTotalLowCentsKwh: applyContractTerms(low, settings, hour)
          .totalCentsKwh,
        estimatedTotalHighCentsKwh: applyContractTerms(high, settings, hour)
          .totalCentsKwh,
      };
    }
    return entry;
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

    const settings = await getUserSettingsFromContext(c);
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
    // fit + lags + the output sanity bound) through the last published quarter,
    // and the four Fingrid series for the full [history, seriesEnd) window.
    const historyStartMs = seriesStartMs - FLOOR_HISTORY_DAYS * DAY_MS;
    const historyStartUtc = new Date(historyStartMs).toISOString();
    const fingridEndUtc = new Date(seriesEndMs).toISOString();

    const seriesStartUtc = new Date(seriesStartMs).toISOString();
    const prices = await getPricesByRange(
      db,
      historyStartUtc,
      seriesStartUtc,
      FORECAST_AREA,
    );
    const spot = spotPricesByKey(prices);

    // Neighbour-area (SE1/SE3/EE) prices over the same history window feed the
    // model's price-lag features. Read in one query; an absent area is
    // neutral-filled downstream and never degrades the response.
    const neighborByArea = await getPricesByAreas(
      db,
      historyStartUtc,
      seriesStartUtc,
      NEIGHBOR_AREAS,
    );
    const neighborPricesByArea = new Map<string, Map<string, number>>();
    for (const [neighborArea, neighborPrices] of neighborByArea) {
      neighborPricesByArea.set(neighborArea, spotPricesByKey(neighborPrices));
    }

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

    // Output-only sanity clamp derived from observed price extremes. NOT a price
    // floor: it is a wide guard against degenerate extrapolation and never
    // re-ties cheap/negative quarters. Omitted entirely when there is no usable
    // history, so the default is an honest no-op.
    const sanityBound = sanityBoundFromHistory(spot);
    const result = buildForecast(
      {
        spotPricesByKey: spot,
        neighborPricesByArea,
        windForecast,
        windActual,
        consumptionForecast,
        consumptionActual,
        seriesStartMs,
        seriesEndMs,
      },
      sanityBound !== null ? { sanityBound } : {},
    );

    const entries = toForecastEntries(result, settings);

    // Degraded/low-confidence iff the model fell back to a default constant
    // (too few aligned samples / singular system) or a hard outage forced
    // zero-seeding. Tail extension and neutral-filled neighbours are NOT
    // degraded — confidence reflects whether the fit had enough to learn from.
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
                ? "Insufficient price history to fit the model; using a default estimate"
                : "Some quarters had no input data and were zero-seeded",
            }
          : {}),
        generatedAt,
        unit: "c/kWh",
        resolutionMinutes: 15,
        bands: bandsDescriptor(),
        entries,
      },
      200 as const,
    );
  });
};

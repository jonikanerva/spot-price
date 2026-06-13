import { z } from "zod";
import type { WeatherFetchResult, WeatherRecord } from "./types.js";

/**
 * OpenWeatherMap One Call API 3.0 fetch boundary for the FI weather collection
 * job (issue #73, Phase 1: forward-only collection; no change to any
 * price/forecast response).
 *
 * Weather data attribution: forecasts are sourced from OpenWeatherMap
 * (https://openweathermap.org) under the Open Data Commons Open Database
 * License (ODbL). spot-price is single-tenant and never redistributes raw
 * weather data — it stores forecasts only to drive its own derived FI price
 * forecast — so this credit is the required attribution.
 *
 * Calls One Call 3.0 for a single point, validates the response with zod
 * (STACK.md forbids raw `fetch` without zod-validated parsing — only the fields
 * the forecast uses are validated; the schema is NOT `.strict()` because OWM
 * adds fields), and degrades gracefully: a timeout, auth error, HTTP error, or
 * malformed body yields an empty `records` array plus a `reason` — it NEVER
 * throws, so a weather problem can never break the authoritative Nord Pool
 * price path.
 *
 * The API key is passed in as a parameter so this module does not touch
 * `process.env` / `env.ts` — the boundary stays a pure function of (key, point,
 * issuedAt), which also keeps it trivial to leave un-exercised in tests that
 * have no key.
 */

const BASE_URL = "https://api.openweathermap.org/data/3.0/onecall";

const REQUEST_TIMEOUT_MS = 30_000;

/** A fixed FI collection point: a stable id plus its coordinates. */
export interface WeatherPoint {
  readonly id: string;
  readonly lat: number;
  readonly lon: number;
}

/**
 * The fixed set of FI points the forecast collects weather for. Deliberately
 * SMALL (devils-advocate scope cut): southern demand/solar centre (Helsinki)
 * plus the west-coast wind region (Vaasa). Two points × 24 hourly runs ≈ 48
 * One Call requests/day — well inside the 1000/day free tier. Adding points is
 * a later, explicit decision (the leakage-free history only accumulates for the
 * points collected from deploy onward).
 */
export const HELSINKI: WeatherPoint = {
  id: "helsinki",
  lat: 60.17,
  lon: 24.94,
};
export const VAASA: WeatherPoint = { id: "vaasa", lat: 63.1, lon: 21.62 };

export const WEATHER_POINTS: readonly WeatherPoint[] = [HELSINKI, VAASA];

/**
 * Boundary schema for a single One Call 3.0 hourly entry. Only the fields the
 * forecast uses are validated; OWM returns more (pressure, humidity, pop, …)
 * and may add others, so this is intentionally NOT `.strict()`.
 */
const HourlyEntrySchema = z.object({
  dt: z.number(),
  temp: z.number(),
  clouds: z.number(),
  uvi: z.number(),
  wind_speed: z.number(),
  wind_deg: z.number(),
});

/** Boundary schema for the One Call 3.0 response envelope. */
const OneCallResponseSchema = z.object({
  hourly: z.array(HourlyEntrySchema),
});

type ParsedOneCall = z.infer<typeof OneCallResponseSchema>;

export interface WeatherFetchParams {
  readonly apiKey: string;
  readonly point: WeatherPoint;
  /**
   * Issuance instant; truncated to the hour (UTC) and stored as `issuedAt` so a
   * later backtest can reconstruct what the forecast said at each issue time.
   */
  readonly issuedAt: Date;
}

const buildUrl = (apiKey: string, point: WeatherPoint): string => {
  const url = new URL(BASE_URL);
  url.searchParams.set("lat", String(point.lat));
  url.searchParams.set("lon", String(point.lon));
  url.searchParams.set("appid", apiKey);
  url.searchParams.set("units", "metric");
  url.searchParams.set("exclude", "current,minutely,daily,alerts");
  return url.toString();
};

/** Truncate a Date to the start of its UTC hour and return the ISO string. */
const issuanceHourIso = (issuedAt: Date): string => {
  const truncated = new Date(issuedAt.getTime());
  truncated.setUTCMinutes(0, 0, 0);
  return truncated.toISOString();
};

/**
 * Pure mapping from a parsed One Call response to weather records. Network-free
 * and unit-testable: `dt` (seconds) becomes a UTC ISO `targetTime`, the
 * issuance is truncated to the hour, and the used fields are passed through.
 */
export const hourlyToRecords = (
  point: WeatherPoint,
  issuedAt: Date,
  parsed: ParsedOneCall,
): readonly WeatherRecord[] => {
  const issuedAtIso = issuanceHourIso(issuedAt);
  return parsed.hourly.map((h) => ({
    pointId: point.id,
    issuedAt: issuedAtIso,
    targetTime: new Date(h.dt * 1000).toISOString(),
    temp: h.temp,
    clouds: h.clouds,
    uvi: h.uvi,
    windSpeed: h.wind_speed,
    windDeg: h.wind_deg,
  }));
};

const degraded = (reason: string): WeatherFetchResult => ({
  ok: false,
  records: [],
  reason,
});

/**
 * Fetch the One Call 3.0 hourly forecast for a single point. Always resolves;
 * failures are reported via the degraded branch of the tagged union and never
 * thrown.
 */
export const fetchWeather = async (
  params: WeatherFetchParams,
): Promise<WeatherFetchResult> => {
  const url = buildUrl(params.apiKey, params.point);
  const controller = new AbortController();
  const timeout = setTimeout(() => {
    controller.abort();
  }, REQUEST_TIMEOUT_MS);

  try {
    const response = await fetch(url, {
      headers: { Accept: "application/json" },
      signal: controller.signal,
    });

    if (response.status === 401 || response.status === 403) {
      return degraded(
        `OpenWeatherMap auth failed (HTTP ${String(response.status)})`,
      );
    }
    if (!response.ok) {
      return degraded(
        `OpenWeatherMap returned HTTP ${String(response.status)}`,
      );
    }

    const body: unknown = await response.json();
    const parsed = OneCallResponseSchema.safeParse(body);
    if (!parsed.success) {
      return degraded("OpenWeatherMap response failed schema validation");
    }

    return {
      ok: true,
      records: hourlyToRecords(params.point, params.issuedAt, parsed.data),
    };
  } catch (error) {
    if (error instanceof Error && error.name === "AbortError") {
      return degraded("OpenWeatherMap request timed out");
    }
    const msg = error instanceof Error ? error.message : "unknown error";
    return degraded(`OpenWeatherMap request failed: ${msg}`);
  } finally {
    clearTimeout(timeout);
  }
};

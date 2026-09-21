import { z } from "zod";
import type {
  WeatherDailyRecord,
  WeatherDailyResult,
  WeatherFetchResult,
  WeatherRecord,
} from "./types.js";

/**
 * OpenWeatherMap One Call API 3.0 fetch boundary for the FI weather collection
 * job.
 *
 * Weather data attribution: forecasts are sourced from OpenWeatherMap
 * (https://openweathermap.org) under the Open Data Commons Open Database
 * License (ODbL). spot-price is single-tenant and never redistributes raw
 * weather data — it stores forecasts only to drive its own derived FI price
 * forecast — so this credit is the required attribution.
 *
 * Validates the response with zod (`STACK.md §7` forbids raw `fetch` without
 * zod-validated parsing). Only the fields the forecast uses are validated, and
 * the schema is NOT `.strict()` because OWM adds fields. Degrades gracefully: a
 * timeout, auth error, HTTP error, or malformed body yields an empty `records`
 * array plus a `reason`. It NEVER throws, so a weather problem can never break
 * the authoritative Nord Pool price path.
 *
 * The hourly and daily blocks are parsed INDEPENDENTLY, so a daily schema drift
 * can never discard the hourly rows.
 *
 * The API key is a parameter, not a `process.env` read, so this boundary stays a
 * pure function of (key, point, issuedAt).
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
 * The fixed set of FI points the forecast collects weather for: the southern
 * demand/solar centre (Helsinki) plus the west-coast wind region (Vaasa).
 * Adding points is a later, explicit decision (the leakage-free history only
 * accumulates for the points collected from deploy onward).
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

/** Boundary schema for the One Call 3.0 response envelope (HOURLY block). */
const OneCallHourlySchema = z.object({
  hourly: z.array(HourlyEntrySchema),
});

type ParsedOneCallHourly = z.infer<typeof OneCallHourlySchema>;

/**
 * Largest absolute UNIX epoch SECONDS value that `new Date(...)` can represent:
 * the ECMA-262 time range is ±8.64e15 ms, so ±8.64e12 s.
 *
 * `z.number()` alone rejects `NaN` and `Infinity` but accepts any other finite
 * number, so a value like `1e13` would pass the schema and then make
 * `toISOString()` throw `RangeError: Invalid time value` in the MAPPING — inside
 * the shared `try` of `fetchWeather`, which would degrade the HOURLY result and
 * discard rows that parsed perfectly. Bounding the epoch fields here keeps that
 * anomaly inside the daily-only degrade path, where it belongs.
 */
const MAX_EPOCH_SECONDS = 8.64e12;

const EpochSecondsSchema = z
  .number()
  .min(-MAX_EPOCH_SECONDS)
  .max(MAX_EPOCH_SECONDS);

/**
 * Boundary schema for a single One Call 3.0 DAILY entry.
 *
 * Collected: all six `temp` sub-fields, `clouds`, `uvi`, and the solar bounds
 * `sunrise` / `sunset`. The solar bounds are what make the daily scalars usable
 * at all: `clouds` and `uvi` are ONE value for the whole day. Bounded by
 * `sunrise`/`sunset` they become a within-day shape instead — a closed-form
 * diurnal curve, which is what `VISION.md → The forecast` allows.
 *
 * Deliberately NOT collected: `wind_speed` / `wind_deg` (Fingrid dataset 245
 * already forecasts wind power at 15-minute resolution over ~72 h for the whole
 * national fleet), `feels_like` (a transform of other fields — a model output,
 * not an observation), and `pop` / `rain` / `snow` / `wind_gust` / `pressure` /
 * `humidity` / `dew_point` / `summary` / the moon fields.
 *
 * `sunrise` / `sunset` are OPTIONAL because One Call omits them at polar
 * latitudes during midnight sun and polar night. Helsinki and Vaasa never hit
 * that, but `WEATHER_POINTS` is documented as extensible, and a schema that
 * assumed the fields would break on the first northern point. Like the hourly
 * schema this is NOT `.strict()`: unknown fields and a changed array length
 * must never fail the parse.
 */
const DailyEntrySchema = z.object({
  dt: EpochSecondsSchema,
  sunrise: EpochSecondsSchema.optional(),
  sunset: EpochSecondsSchema.optional(),
  temp: z.object({
    morn: z.number(),
    day: z.number(),
    eve: z.number(),
    night: z.number(),
    min: z.number(),
    max: z.number(),
  }),
  clouds: z.number(),
  uvi: z.number(),
});

/** Boundary schema for the One Call 3.0 response envelope (DAILY block). */
const OneCallDailySchema = z.object({
  daily: z.array(DailyEntrySchema),
});

type ParsedOneCallDaily = z.infer<typeof OneCallDailySchema>;

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
  // Never add `daily` to `exclude`: the daily block rides along in the SAME
  // response and costs no extra call (`STACK.md §9`).
  url.searchParams.set("exclude", "current,minutely,alerts");
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
  parsed: ParsedOneCallHourly,
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

/** UNIX epoch seconds to a UTC ISO instant; `undefined` stays absent as null. */
const epochSecondsToIso = (seconds: number | undefined): string | null =>
  seconds === undefined ? null : new Date(seconds * 1000).toISOString();

/**
 * Pure mapping from a parsed One Call DAILY block to daily records.
 * Network-free and unit-testable.
 *
 * `targetDate` is the UTC CALENDAR DATE of `dt` — the first ten characters of
 * the UTC ISO instant. No `Intl`, no timezone lookup, no local-time arithmetic:
 * `STACK.md §7` forbids that below the response boundary, and `daily[].dt` is
 * local noon at the point, so a "day" defined by local reasoning would drag a
 * timezone (and the DST fall-back) into storage. Deriving in UTC is exact for
 * every point between UTC−11 and UTC+11, which covers all of FI with a wide
 * margin. The raw `dt` is stored alongside as `targetDt`, so the derivation
 * stays recomputable without re-collecting.
 */
export const dailyToRecords = (
  point: WeatherPoint,
  issuedAt: Date,
  parsed: ParsedOneCallDaily,
): readonly WeatherDailyRecord[] => {
  const issuedAtIso = issuanceHourIso(issuedAt);
  return parsed.daily.map((d) => {
    const targetDt = new Date(d.dt * 1000).toISOString();
    return {
      pointId: point.id,
      issuedAt: issuedAtIso,
      targetDate: targetDt.slice(0, 10),
      targetDt,
      tempMorn: d.temp.morn,
      tempDay: d.temp.day,
      tempEve: d.temp.eve,
      tempNight: d.temp.night,
      tempMin: d.temp.min,
      tempMax: d.temp.max,
      clouds: d.clouds,
      uvi: d.uvi,
      sunrise: epochSecondsToIso(d.sunrise),
      sunset: epochSecondsToIso(d.sunset),
    };
  });
};

const dailyDegraded = (reason: string): WeatherDailyResult => ({
  ok: false,
  records: [],
  reason,
});

/**
 * Parse the DAILY block of an already-fetched body, INDEPENDENTLY of the hourly
 * parse. Two separate `safeParse` calls over the same body is the load-bearing
 * shape: folding `daily` into the hourly schema would mean one deviating daily
 * entry fails the whole parse, so the point's HOURLY rows are discarded — and
 * `weather-job.ts` records that an issuance can never be re-fetched once its
 * hour has passed. The failure would also look like ordinary per-point
 * degradation rather than a regression.
 *
 * No `.catch()` default and no coercion: a schema drift must surface as a
 * degraded daily result with a reason, never be silently papered over. The
 * try/catch around the MAPPING is not such a default — it reports the error
 * message as the degrade reason. It is a structural guarantee that NOTHING on
 * the daily path can throw inside the shared `try` of `fetchWeather`, where a
 * throw would degrade the hourly result and discard rows that parsed fine.
 * `EpochSecondsSchema` already removes the one known way to get there
 * (`RangeError` from an out-of-range epoch); this keeps the guarantee true for
 * any future mapping change instead of resting on an argument.
 */
const parseDaily = (
  point: WeatherPoint,
  issuedAt: Date,
  body: unknown,
): WeatherDailyResult => {
  const parsed = OneCallDailySchema.safeParse(body);
  if (!parsed.success) {
    return dailyDegraded("OpenWeatherMap daily block failed schema validation");
  }
  try {
    return { ok: true, records: dailyToRecords(point, issuedAt, parsed.data) };
  } catch (error) {
    const msg = error instanceof Error ? error.message : "unknown error";
    return dailyDegraded(`OpenWeatherMap daily block failed mapping: ${msg}`);
  }
};

const degraded = (reason: string): WeatherFetchResult => ({
  ok: false,
  records: [],
  reason,
  daily: dailyDegraded(reason),
});

/**
 * Fetch the One Call 3.0 forecast for a single point — the hourly block and the
 * daily block from the same response. Always resolves; failures are reported
 * via the degraded branch of the tagged union and never thrown. The two blocks
 * degrade independently.
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

    // ONE body, TWO independent parses — see `parseDaily`.
    const body: unknown = await response.json();
    const daily = parseDaily(params.point, params.issuedAt, body);

    const parsed = OneCallHourlySchema.safeParse(body);
    if (!parsed.success) {
      return {
        ...degraded("OpenWeatherMap response failed schema validation"),
        daily,
      };
    }

    return {
      ok: true,
      records: hourlyToRecords(params.point, params.issuedAt, parsed.data),
      daily,
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

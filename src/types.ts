/** Parsed hourly price in our domain */
export interface HourlyPrice {
  readonly deliveryStart: string;
  readonly deliveryEnd: string;
  readonly priceEurMwh: number;
  readonly area: string;
}

/** User electricity contract settings */
export interface UserSettings {
  readonly userId: string;
  readonly marginCentsKwh: number;
  readonly transferDayCentsKwh: number;
  readonly transferNightCentsKwh: number;
  readonly taxCentsKwh: number;
  readonly vatPercent: number;
  readonly nightStartHour: number;
  readonly nightEndHour: number;
  readonly timezone: string;
  readonly area: string;
}

/** Total price breakdown for a single delivery interval */
export interface TotalPrice {
  readonly deliveryStart: string;
  readonly deliveryEnd: string;
  readonly localStart: string;
  readonly localEnd: string;
  readonly spotCentsKwh: number;
  readonly marginCentsKwh: number;
  readonly transferCentsKwh: number;
  readonly taxCentsKwh: number;
  readonly vatCentsKwh: number;
  readonly totalCentsKwh: number;
  readonly isNightRate: boolean;
}

/** Window result */
export interface PriceWindow {
  readonly start: string;
  readonly end: string;
  readonly startLocal: string;
  readonly endLocal: string;
  readonly minTotalCentsKwh: number;
  readonly maxTotalCentsKwh: number;
  readonly averageTotalCentsKwh: number;
  readonly prices: TotalPrice[];
}

// ---------------------------------------------------------------------------
// Forecast feature (FI only) — Fingrid grid data + closed-form estimate
// ---------------------------------------------------------------------------

/**
 * A single observation from a Fingrid Open Data dataset, normalised to our
 * domain. Times are UTC ISO 8601 strings; `value` is the raw dataset unit
 * (MW for wind, MW for consumption). These are public grid data, not user data
 * (`VISION.md → Persistence and Privacy Posture`).
 */
export interface FingridRecord {
  readonly datasetId: number;
  readonly startTime: string;
  readonly endTime: string;
  readonly value: number;
}

/**
 * One archived per-issuance vintage of a Fingrid FORECAST dataset (245/165): a
 * `FingridRecord` plus the `issuedAt` at which that value was recorded (the
 * hour-truncated fetch-time proxy from migration 005). Public grid data, not
 * user data. Read only off the request path, by the offline studies that need
 * the full lead-time ladder. The live forecast route uses the latest-per-target
 * read instead.
 */
export interface ForecastVintageRecord extends FingridRecord {
  readonly issuedAt: string;
}

/**
 * Result of a Fingrid fetch. Tagged union over success/degraded so the caller
 * never has to inspect parallel booleans: a failure (timeout, auth, parse)
 * yields an empty `records` plus a `reason`, and never throws into the price
 * path.
 */
export type FingridFetchResult =
  | { readonly ok: true; readonly records: readonly FingridRecord[] }
  | {
      readonly ok: false;
      readonly records: readonly FingridRecord[];
      readonly reason: string;
    };

/**
 * One issued OpenWeatherMap hourly weather forecast, normalised to our domain.
 * Times are UTC ISO 8601 strings: `issuedAt` is the issuance hour (when the
 * forecast was fetched, truncated to the hour) and `targetTime` is the hour the
 * values describe. Storing BOTH is what makes a later leakage-free
 * weather-feature backtest possible — the row records what the forecast said at
 * issue time, never a hindsight-overwritten value. These are public weather
 * data, not user data (`VISION.md → Persistence and Privacy Posture`).
 */
export interface WeatherRecord {
  readonly pointId: string;
  readonly issuedAt: string;
  readonly targetTime: string;
  readonly temp: number;
  readonly clouds: number;
  readonly uvi: number;
  readonly windSpeed: number;
  readonly windDeg: number;
}

/**
 * One issued OpenWeatherMap DAILY weather forecast, normalised to our domain.
 * `issuedAt` is the issuance hour, as for `WeatherRecord`.
 *
 * `targetDate` is the UTC CALENDAR DATE (`YYYY-MM-DD`) of `targetDt`, and
 * `targetDt` is the raw upstream `dt` kept as provenance so the date derivation
 * stays recomputable without re-collecting. The date is derived in UTC only —
 * `STACK.md §7` forbids local-time arithmetic below the response boundary — so
 * for a collection point east of UTC+11 or west of UTC−11 the derived date could
 * differ from the point's local calendar day. Every point in `WEATHER_POINTS`
 * is FI, far inside that range.
 *
 * `sunrise` / `sunset` are nullable on purpose: One Call omits them at polar
 * latitudes during midnight sun and polar night. Public weather data, not user
 * data (`VISION.md → Persistence and Privacy Posture`).
 */
export interface WeatherDailyRecord {
  readonly pointId: string;
  readonly issuedAt: string;
  readonly targetDate: string;
  readonly targetDt: string;
  readonly tempMorn: number;
  readonly tempDay: number;
  readonly tempEve: number;
  readonly tempNight: number;
  readonly tempMin: number;
  readonly tempMax: number;
  readonly clouds: number;
  readonly uvi: number;
  readonly sunrise: string | null;
  readonly sunset: string | null;
}

/**
 * Outcome of parsing the DAILY block of one One Call response. Tagged
 * separately from the hourly outcome, and nested inside `WeatherFetchResult`
 * rather than flattened into parallel booleans, because the two blocks are
 * parsed INDEPENDENTLY: a daily-schema drift must degrade only itself and leave
 * the hourly records untouched, since an issuance can never be re-fetched once
 * its hour has passed.
 */
export type WeatherDailyResult =
  | { readonly ok: true; readonly records: readonly WeatherDailyRecord[] }
  | {
      readonly ok: false;
      readonly records: readonly WeatherDailyRecord[];
      readonly reason: string;
    };

/**
 * Result of a weather fetch for a single point. Tagged union over
 * success/degraded so the caller never inspects parallel booleans: a failure
 * (timeout, auth, parse) yields an empty `records` plus a `reason`, and never
 * throws — a weather problem can never reach the authoritative price path.
 *
 * `daily` carries the independently-parsed daily block on BOTH branches: a
 * transport failure degrades both, while a schema failure in one block leaves
 * the other intact.
 */
export type WeatherFetchResult =
  | {
      readonly ok: true;
      readonly records: readonly WeatherRecord[];
      readonly daily: WeatherDailyResult;
    }
  | {
      readonly ok: false;
      readonly records: readonly WeatherRecord[];
      readonly reason: string;
      readonly daily: WeatherDailyResult;
    };

/**
 * Daily-block summary of one weather job run. Kept OUTSIDE the `status` tag on
 * purpose: `status` stays a statement about the HOURLY collection, so the
 * "every point failed → do not prune" guard keeps its meaning. A daily failure
 * never downgrades an hourly success.
 */
export interface WeatherDailyJobSummary {
  /** Daily rows inserted this run (append-only per issuance). */
  readonly stored: number;
  /** Points whose daily block degraded, and why. */
  readonly failures: readonly WeatherPointFailure[];
}

/**
 * Outcome of the hourly weather fetch job across all configured points. Tagged
 * by completeness so the caller distinguishes full success / partial (some
 * points degraded) / total failure without throwing. `failures` names the
 * points that degraded and why; `stored`/`pruned` are aggregate row counts of
 * the HOURLY collection, and `daily` reports the daily block separately.
 */
export type WeatherFetchJobResult =
  | {
      readonly status: "ok";
      readonly stored: number;
      readonly pruned: number;
      readonly daily: WeatherDailyJobSummary;
    }
  | {
      readonly status: "partial";
      readonly stored: number;
      readonly pruned: number;
      readonly failures: readonly WeatherPointFailure[];
      readonly daily: WeatherDailyJobSummary;
    }
  | {
      readonly status: "failed";
      readonly failures: readonly WeatherPointFailure[];
      /**
       * Present on this branch too: the daily block is parsed independently, so
       * an hourly schema drift can leave valid daily rows even when EVERY point
       * fails on the hourly side.
       */
      readonly daily: WeatherDailyJobSummary;
    };

/** A single point's degraded outcome within a weather fetch job run. */
export interface WeatherPointFailure {
  readonly pointId: string;
  readonly reason: string;
}

/**
 * One predicted quarter of the forecast series. Money fields are deliberately
 * named `estimated*` so they share NO field name with the real-price schemas
 * (`TotalPrice`) — a misrouted consumer cannot blind-read an estimate as a
 * published price. The response's top-level `forecast: true` flags the whole
 * payload as an estimate.
 */
export interface ForecastEntry {
  readonly start: string;
  readonly end: string;
  readonly localStart: string;
  readonly localEnd: string;
  readonly estimatedSpotCentsKwh: number;
  readonly estimatedTotalCentsKwh: number;
  /**
   * Optional empirical prediction-band bounds (P10/P90-style). Present only when
   * a calibrated band artifact ships AND the quarter carries a real prediction
   * (not forward-filled / zero-seeded). Additive, v1-compatible: absent ⇒ no
   * band. Each bound is the point's contract-applied bound (low ≤ point ≤ high).
   */
  readonly estimatedSpotLowCentsKwh?: number;
  readonly estimatedSpotHighCentsKwh?: number;
  readonly estimatedTotalLowCentsKwh?: number;
  readonly estimatedTotalHighCentsKwh?: number;
}

/**
 * Diagnostics from the forecast pipeline. `fitUsedDefault` and
 * `zeroSeededQuarters` drive the `degraded`/`confidence` signal: the forecast
 * is only honest if it says so when the model fell back to a default constant
 * or had to zero-seed a hard data outage. Tail extension alone is NOT degraded.
 * Internal only — never serialised into the API response.
 */
export interface ForecastDiagnostics {
  /** Aligned training samples the model fit actually used. */
  readonly fitSamples: number;
  /** Number of feature columns the model fit over. */
  readonly featureCount: number;
  readonly fitUsedDefault: boolean;
  readonly consumptionExtendedQuarters: number;
  readonly windExtendedQuarters: number;
  readonly filledQuarters: number;
  readonly zeroSeededQuarters: number;
  /**
   * Predicted quarters that had no 1d/7d persistence lag to take a within-day
   * shape from and fell back to the flat per-day ridge level. Near-zero in
   * practice (the route reads ~30d of contiguous history); a non-trivial count
   * at the far horizon would mean the shape is thinning out. NOT a degrade
   * trigger — a flat-level fallback is still an honest level estimate.
   */
  readonly shapeFallbackQuarters: number;
  /**
   * Predicted quarters clamped by the output sanity bound (`sanityBoundFromHistory`).
   * Expected 0 in practice — the bound sits provably outside the real-data range,
   * so a non-zero count means the model extrapolated absurdly far and the clamp
   * caught it. NOT a price floor and never re-ties cheap/negative quarters.
   */
  readonly sanityClampedQuarters: number;
  /** True when a calibrated band artifact was applied to the series. */
  readonly bandCalibrated: boolean;
  /** Number of future quarters that received band bounds. */
  readonly bandHourBuckets: number;
}

/** Pure-pipeline output: the predicted spot series plus diagnostics. */
export interface ForecastResult {
  readonly series: readonly ForecastSpotPoint[];
  readonly diagnostics: ForecastDiagnostics;
}

/**
 * A single predicted SPOT-price quarter from the pure pipeline, before contract
 * terms are applied. UTC ISO quarter key + estimated spot c/kWh.
 */
export interface ForecastSpotPoint {
  readonly start: string;
  readonly estimatedSpotCentsKwh: number;
  /**
   * Optional empirical band bounds in spot c/kWh, written by `buildForecast`
   * only for quarters carrying a real prediction when a calibrated band ships.
   * Forward-filled / zero-seeded quarters never get a band. `low ≤ point ≤ high`.
   */
  readonly estimatedSpotLowCentsKwh?: number;
  readonly estimatedSpotHighCentsKwh?: number;
}

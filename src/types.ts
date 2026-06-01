/** Hourly price entry from Nord Pool Data Portal API */
export interface NordPoolEntry {
  readonly deliveryStart: string;
  readonly deliveryEnd: string;
  readonly entryPerArea: Readonly<Record<string, number>>;
}

/** Nord Pool Data Portal API response */
export interface NordPoolResponse {
  readonly deliveryDateCET: string;
  readonly updatedAt: string;
  readonly currency: string;
  readonly multiAreaEntries: readonly NordPoolEntry[];
  readonly areaStates: readonly {
    readonly state: string;
    readonly areas: readonly string[];
  }[];
}

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
 * One predicted quarter of the forecast series. Money fields are deliberately
 * named `estimated*` and carry `estimated: true` so they share NO field name
 * with the real-price schemas (`TotalPrice`) — a misrouted consumer cannot
 * blind-read an estimate as a published price.
 */
export interface ForecastEntry {
  readonly start: string;
  readonly end: string;
  readonly localStart: string;
  readonly localEnd: string;
  readonly estimatedSpotCentsKwh: number;
  readonly estimatedTotalCentsKwh: number;
  readonly estimated: true;
}

/**
 * Diagnostics from the forecast pipeline. `fitUsedDefault` and
 * `zeroSeededQuarters` drive the `degraded`/`confidence` signal: the forecast
 * is only honest if it says so when it fell back to default coefficients or had
 * to zero-seed a hard data outage. Tail extension alone is NOT degraded.
 */
export interface ForecastDiagnostics {
  readonly slope: number;
  readonly intercept: number;
  readonly fitSamples: number;
  readonly fitUsedDefault: boolean;
  readonly consumptionExtendedQuarters: number;
  readonly windExtendedQuarters: number;
  readonly filledQuarters: number;
  readonly zeroSeededQuarters: number;
  readonly hourBiasBuckets: number;
  readonly predictionFloor: number | null;
  readonly floorClippedQuarters: number;
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
}

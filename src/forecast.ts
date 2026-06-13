import type {
  FingridRecord,
  ForecastDiagnostics,
  ForecastResult,
  ForecastSpotPoint,
} from "./types.js";
import {
  buildFeatureVectorIndexed,
  buildForecastPriceIndex,
  buildTrainingMatrix,
  type FeatureContext,
} from "./features.js";
import { createRidgeModel, type Model } from "./model.js";

/**
 * Pure FI price-forecast pipeline. A pluggable closed-form regression over a
 * rich feature set (grid residual + wind/consumption + an interaction term, FI
 * and neighbour price lags, UTC calendar) — still no machine learning: the
 * default `Model` is hand-rolled ridge (`model.ts`), and a per-hour bias plus
 * an optional price floor refine the level afterwards.
 *
 * This module is strictly pure: no `pg`, no `fetch`, no `env`, no `Date.now()`.
 * Everything works on UTC ISO 8601 quarter keys (15-min floor). The I/O
 * boundary (the route) supplies the time window, the prices (FI + neighbours),
 * the Fingrid series, and the history slice for the floor; this module computes
 * the estimate. It predicts SPOT c/kWh; the fit target is the caller's stored
 * spot price (already converted to c/kWh).
 */

// ---------------------------------------------------------------------------
// Tunable constants
// ---------------------------------------------------------------------------

/** Wind has no weekly cycle, so the tail averages several weeks toward climatology. */
export const WIND_EXTENSION_WEEKS = 4;
/** Consumption has a strong Finnish weekly cycle, so one week back is most representative. */
export const CONSUMPTION_EXTENSION_WEEKS = 1;
/** Days of stored price history the floor percentile is computed over (~1 month, single season). */
export const FLOOR_HISTORY_DAYS = 30;
/** Percentile of hourly price minima used as the robust lower clip. */
export const FLOOR_PERCENTILE = 5;
/** Predicted series length: fixed N days after the last published price. */
export const FORECAST_DAYS = 3;

const QUARTER_MS = 15 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;
const WEEK_MS = 7 * DAY_MS;

// ---------------------------------------------------------------------------
// Time helpers (UTC ISO quarter keys)
// ---------------------------------------------------------------------------

/** Floor a UTC instant (ms) down to its 15-min quarter boundary. */
export const quarterFloorUtc = (ms: number): number =>
  Math.floor(ms / QUARTER_MS) * QUARTER_MS;

/** Canonical UTC ISO quarter key for a UTC instant (ms). */
export const quarterKey = (ms: number): string =>
  new Date(quarterFloorUtc(ms)).toISOString();

/** UTC hour-of-day (0-23) of a quarter key. */
const hourOfKey = (key: string): number => new Date(key).getUTCHours();

// ---------------------------------------------------------------------------
// Bucketing
// ---------------------------------------------------------------------------

/**
 * Bucket Fingrid records by 15-min quarter, taking the mean when multiple
 * samples land in the same quarter. Malformed records (non-finite value or
 * unparseable time) are skipped — one bad row never breaks the pipeline.
 */
export const bucketRecords = (
  records: readonly FingridRecord[],
): ReadonlyMap<string, number> => {
  const sums = new Map<string, number>();
  const counts = new Map<string, number>();
  for (const r of records) {
    const ms = new Date(r.startTime).getTime();
    if (!Number.isFinite(ms) || !Number.isFinite(r.value)) {
      continue;
    }
    const key = quarterKey(ms);
    sums.set(key, (sums.get(key) ?? 0) + r.value);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const out = new Map<string, number>();
  for (const [key, sum] of sums) {
    const count = counts.get(key) ?? 1;
    out.set(key, sum / count);
  }
  return out;
};

/**
 * Bucket records to 15-min quarters, filling any hour that only carries its
 * top-of-hour (`:00`) value — i.e. genuinely hourly input (historically
 * dataset 124). Resolution-agnostic: genuine 15-min input passes through with
 * its distinct values intact.
 */
export const expandHourlyToQuarters = (
  records: readonly FingridRecord[],
): ReadonlyMap<string, number> => {
  const quarters = bucketRecords(records);
  const out = new Map(quarters);
  for (const [key, value] of quarters) {
    const ms = new Date(key).getTime();
    if (new Date(ms).getUTCMinutes() === 0) {
      for (const offsetQuarters of [1, 2, 3]) {
        const fillKey = quarterKey(ms + offsetQuarters * QUARTER_MS);
        if (!quarters.has(fillKey)) {
          out.set(fillKey, value);
        }
      }
    }
  }
  return out;
};

// ---------------------------------------------------------------------------
// Tail extension (same weekday/quarter, N weeks back)
// ---------------------------------------------------------------------------

/**
 * Fill quarters after `forecast` ends, up to `seriesEndMs` (exclusive), with
 * the mean of the same weekday/quarter over the last `weeks` weeks taken from
 * `actual`. Returns a new map (forecast values preserved). Quarters with no
 * usable history in any look-back are left unfilled.
 */
export const extendWithLastWeek = (
  forecast: ReadonlyMap<string, number>,
  actual: ReadonlyMap<string, number>,
  seriesEndMs: number,
  weeks: number,
): ReadonlyMap<string, number> => {
  const out = new Map(forecast);
  if (forecast.size === 0) {
    return out;
  }
  let lastKnownMs = -Infinity;
  for (const key of forecast.keys()) {
    const ms = new Date(key).getTime();
    if (ms > lastKnownMs) {
      lastKnownMs = ms;
    }
  }
  let cursor = lastKnownMs + QUARTER_MS;
  while (cursor < seriesEndMs) {
    const values: number[] = [];
    for (let week = 1; week <= weeks; week++) {
      const prevKey = quarterKey(cursor - week * WEEK_MS);
      const prev = actual.get(prevKey);
      if (prev !== undefined) {
        values.push(prev);
      }
    }
    if (values.length > 0) {
      const mean = values.reduce((a, b) => a + b, 0) / values.length;
      out.set(quarterKey(cursor), mean);
    }
    cursor += QUARTER_MS;
  }
  return out;
};

// ---------------------------------------------------------------------------
// Linear fit (closed-form 2-parameter OLS)
// ---------------------------------------------------------------------------

/** Aligned (x, y) pairs over the quarter keys present in both maps. */
export const alignSeries = (
  priceByKey: ReadonlyMap<string, number>,
  residualByKey: ReadonlyMap<string, number>,
): { readonly xs: readonly number[]; readonly ys: readonly number[] } => {
  const common = [...priceByKey.keys()]
    .filter((k) => residualByKey.has(k))
    .sort();
  const xs: number[] = [];
  const ys: number[] = [];
  for (const key of common) {
    const x = residualByKey.get(key);
    const y = priceByKey.get(key);
    if (x !== undefined && y !== undefined) {
      xs.push(x);
      ys.push(y);
    }
  }
  return { xs, ys };
};

/**
 * Closed-form 2-parameter OLS `y = a*x + b`. Throws on fewer than two matching
 * points or zero variance in x — the caller then falls back to defaults.
 */
export const fitLinear = (
  xs: readonly number[],
  ys: readonly number[],
): { readonly slope: number; readonly intercept: number } => {
  const n = xs.length;
  if (n < 2 || n !== ys.length) {
    throw new Error("Need >= 2 matching points to fit.");
  }
  let sx = 0;
  let sy = 0;
  let sxx = 0;
  let sxy = 0;
  for (let i = 0; i < n; i++) {
    const x = xs[i] ?? 0;
    const y = ys[i] ?? 0;
    sx += x;
    sy += y;
    sxx += x * x;
    sxy += x * y;
  }
  const denom = n * sxx - sx * sx;
  if (denom === 0) {
    throw new Error("Zero variance in x; cannot fit.");
  }
  const slope = (n * sxy - sx * sy) / denom;
  const intercept = (sy - slope * sx) / n;
  return { slope, intercept };
};

// ---------------------------------------------------------------------------
// Per-UTC-hour bias correction
// ---------------------------------------------------------------------------

/**
 * Mean residual error (actual - predicted) per UTC hour-of-day over the
 * overlap. The linear model captures the price *level* but not the daily price
 * *rhythm* (peaks/troughs driven by neighbour prices, solar/demand) — this
 * additive per-hour correction recovers that rhythm from the published prices.
 * Returns the per-hour bias plus a `globalBias` fallback for unseen hours.
 */
export const fitHourBias = (
  actualPrices: ReadonlyMap<string, number>,
  predicted: ReadonlyMap<string, number>,
): {
  readonly biasByHour: ReadonlyMap<number, number>;
  readonly globalBias: number;
} => {
  const errorsByHour = new Map<number, number[]>();
  const allErrors: number[] = [];
  for (const [key, actual] of actualPrices) {
    const pred = predicted.get(key);
    if (pred === undefined) {
      continue;
    }
    const error = actual - pred;
    const hour = hourOfKey(key);
    const bucket = errorsByHour.get(hour) ?? [];
    bucket.push(error);
    errorsByHour.set(hour, bucket);
    allErrors.push(error);
  }
  const globalBias =
    allErrors.length > 0
      ? allErrors.reduce((a, b) => a + b, 0) / allErrors.length
      : 0;
  const biasByHour = new Map<number, number>();
  for (const [hour, errs] of errorsByHour) {
    biasByHour.set(hour, errs.reduce((a, b) => a + b, 0) / errs.length);
  }
  return { biasByHour, globalBias };
};

/** Add the per-UTC-hour bias to each predicted quarter (global fallback). */
export const applyHourBias = (
  predicted: ReadonlyMap<string, number>,
  biasByHour: ReadonlyMap<number, number>,
  globalBias: number,
): Map<string, number> => {
  const out = new Map<string, number>();
  for (const [key, value] of predicted) {
    out.set(key, value + (biasByHour.get(hourOfKey(key)) ?? globalBias));
  }
  return out;
};

// ---------------------------------------------------------------------------
// Price floor
// ---------------------------------------------------------------------------

/**
 * Compute the prediction floor from a history slice: the FLOOR_PERCENTILE-th
 * percentile of hourly price minima. Pure — the caller passes in the trailing
 * ~FLOOR_HISTORY_DAYS of stored spot prices as `(quarterKey, spotCentsKwh)`
 * pairs. Returns null when there is no usable history (no clipping applied).
 *
 * A robust lower bound that prevents OLS extrapolation below the observed price
 * range; a 30-day window tracks Finnish seasonal variation without mixing
 * seasons.
 */
export const priceFloorFromHistory = (
  history: ReadonlyMap<string, number>,
  percentile: number = FLOOR_PERCENTILE,
): number | null => {
  const minByHour = new Map<string, number>();
  for (const [key, price] of history) {
    const ms = new Date(key).getTime();
    if (!Number.isFinite(ms)) {
      continue;
    }
    const hourKey = new Date(
      Math.floor(ms / DAY_MS) * DAY_MS + hourOfKey(key) * 3_600_000,
    ).toISOString();
    const current = minByHour.get(hourKey);
    if (current === undefined || price < current) {
      minByHour.set(hourKey, price);
    }
  }
  const mins = [...minByHour.values()].sort((a, b) => a - b);
  if (mins.length === 0) {
    return null;
  }
  const idx = Math.max(0, Math.trunc((mins.length * percentile) / 100) - 1);
  return mins[idx] ?? null;
};

// ---------------------------------------------------------------------------
// Series assembly
// ---------------------------------------------------------------------------

interface BuiltSeries {
  readonly series: readonly ForecastSpotPoint[];
  readonly filledQuarters: number;
  readonly zeroSeededQuarters: number;
}

/**
 * Build a quarter-by-quarter predicted-only list for
 * [seriesStartMs, seriesStartMs + numQuarters * 15min).
 *
 * Invariant: exactly `numQuarters` entries, chronological, no gaps, no nulls.
 * When `predicted` does not cover a quarter, forward-fill from the most recent
 * predicted value (data thinning). If the leading quarters have no predicted
 * value either, look ahead for the first available value to seed; if none
 * exists anywhere, zero-seed (a hard outage — surfaces via `degraded`).
 */
export const buildPredictedSeries = (
  predicted: ReadonlyMap<string, number>,
  seriesStartMs: number,
  numQuarters: number,
): BuiltSeries => {
  const keys: string[] = [];
  for (let i = 0; i < numQuarters; i++) {
    keys.push(quarterKey(seriesStartMs + i * QUARTER_MS));
  }

  let seed: number | null = null;
  for (const key of keys) {
    const value = predicted.get(key);
    if (value !== undefined) {
      seed = value;
      break;
    }
  }

  const series: ForecastSpotPoint[] = [];
  let lastPredicted: number | null = seed;
  let filledQuarters = 0;
  let zeroSeededQuarters = 0;
  for (const key of keys) {
    const value = predicted.get(key);
    if (value !== undefined) {
      lastPredicted = value;
      series.push({
        start: key,
        estimatedSpotCentsKwh: Math.round(value * 1000) / 1000,
      });
      continue;
    }
    if (lastPredicted !== null) {
      series.push({
        start: key,
        estimatedSpotCentsKwh: Math.round(lastPredicted * 1000) / 1000,
      });
      filledQuarters++;
      continue;
    }
    series.push({ start: key, estimatedSpotCentsKwh: 0 });
    zeroSeededQuarters++;
  }
  return { series, filledQuarters, zeroSeededQuarters };
};

// ---------------------------------------------------------------------------
// Top-level pipeline
// ---------------------------------------------------------------------------

export interface ForecastInput {
  /** Published FI spot prices as (UTC ISO quarter key, spot c/kWh) — fit target + lag source. */
  readonly spotPricesByKey: ReadonlyMap<string, number>;
  /**
   * Neighbour-area spot prices (c/kWh) as area → (quarter key → price). Used as
   * lag features only; an entirely-absent area is neutral-filled and is NEVER a
   * degraded trigger. Optional so existing callers/tests need not supply it.
   */
  readonly neighborPricesByArea?: ReadonlyMap<
    string,
    ReadonlyMap<string, number>
  >;
  readonly windForecast: readonly FingridRecord[];
  readonly windActual: readonly FingridRecord[];
  readonly consumptionForecast: readonly FingridRecord[];
  readonly consumptionActual: readonly FingridRecord[];
  /** First quarter to predict (UTC ms) — one quarter after the last published price. */
  readonly seriesStartMs: number;
  /** Exclusive end of the predicted window (UTC ms). */
  readonly seriesEndMs: number;
}

export interface ForecastOptions {
  readonly windExtensionWeeks?: number;
  readonly consumptionExtensionWeeks?: number;
  readonly applyTimeBias?: boolean;
  /** Lower clip for predicted spot c/kWh; null/undefined disables clipping. */
  readonly floor?: number | null;
  /** Ridge L2 penalty forwarded to the model fit. */
  readonly ridgeLambda?: number;
  /**
   * The estimator. Defaults to the closed-form ridge model. Injectable so the
   * backtest can swap models; the interface is type-pinned to closed-form /
   * synchronous / pure estimators (`model.ts` Phase-3 pin).
   */
  readonly model?: Model;
}

/**
 * Run the full pipeline at 15-min resolution, returning the predicted spot
 * series over [seriesStart, seriesEnd) plus diagnostics. The series is entirely
 * predicted — published prices are used to fit the model but are never passed
 * back through (the caller sets `seriesStart` to one quarter after the last
 * published price).
 *
 * Pipeline: build the feature matrix (`features.ts`) over the bounded training
 * window → fit the model → predict each future quarter (the model `sinh`-inverts
 * its arcsinh target transform internally, so predictions are already raw
 * c/kWh) → existing per-UTC-hour bias → existing price-floor clip →
 * `buildPredictedSeries`. Stays pure — the model and feature builders take no
 * I/O.
 */
export const buildForecast = (
  input: ForecastInput,
  opts: ForecastOptions = {},
): ForecastResult => {
  const windWeeks = opts.windExtensionWeeks ?? WIND_EXTENSION_WEEKS;
  const consWeeks =
    opts.consumptionExtensionWeeks ?? CONSUMPTION_EXTENSION_WEEKS;
  const applyTimeBias = opts.applyTimeBias ?? true;
  const floor = opts.floor ?? null;
  const model = opts.model ?? createRidgeModel();

  const seriesStartMs = quarterFloorUtc(input.seriesStartMs);
  const seriesEndMs = quarterFloorUtc(input.seriesEndMs);

  const windQ = bucketRecords(input.windForecast);
  const windActualQ = bucketRecords(input.windActual);
  const consQ = bucketRecords(input.consumptionForecast);
  const consActualQ = expandHourlyToQuarters(input.consumptionActual);

  // Consumption keeps the single-week copy (strong weekly cycle); wind averages
  // several weeks toward climatology (no weekly cycle).
  const consExtended = extendWithLastWeek(
    consQ,
    consActualQ,
    seriesEndMs,
    consWeeks,
  );
  const windExtended = extendWithLastWeek(
    windQ,
    windActualQ,
    seriesEndMs,
    windWeeks,
  );

  // Wind/consumption actuals merged under the (tail-extended) forecast so the
  // feature builder sees a continuous grid series across history + horizon. The
  // forecast value wins where both exist (the future is forecast-only anyway).
  const windByKey = new Map<string, number>(windActualQ);
  for (const [k, v] of windExtended) {
    windByKey.set(k, v);
  }
  const consumptionByKey = new Map<string, number>(consActualQ);
  for (const [k, v] of consExtended) {
    consumptionByKey.set(k, v);
  }

  const ctx: FeatureContext = {
    fiPricesByKey: input.spotPricesByKey,
    neighborPricesByArea: input.neighborPricesByArea ?? new Map(),
    windByKey,
    consumptionByKey,
  };

  // Precompute the price index once and share it across training + every
  // predicted quarter, so feature assembly stays O(1) per quarter (keeps the
  // whole pipeline on the request path under the STACK §4 budget).
  const priceIndex = buildForecastPriceIndex(ctx);

  // Fit on the bounded training window; the model falls back to a neutral
  // constant when there are too few aligned samples or the system is singular.
  const trainOpts =
    opts.ridgeLambda !== undefined ? { ridgeLambda: opts.ridgeLambda } : {};
  const training = buildTrainingMatrix(seriesStartMs, ctx, priceIndex);
  const fitted = model.fit(
    { features: training.features, targets: training.targets },
    trainOpts,
  );
  const fitUsedDefault = fitted.meta.usedFallback;

  const numQuarters = Math.max(
    0,
    Math.trunc((seriesEndMs - seriesStartMs) / QUARTER_MS),
  );

  // Predict each future quarter. The model returns raw c/kWh (arcsinh inverted).
  let predicted = new Map<string, number>();
  for (let i = 0; i < numQuarters; i++) {
    const ms = seriesStartMs + i * QUARTER_MS;
    predicted.set(
      quarterKey(ms),
      fitted.predict(buildFeatureVectorIndexed(ms, ctx, priceIndex)),
    );
  }

  let hourBiasBuckets = 0;
  if (applyTimeBias) {
    // Recover the daily price rhythm the level model misses, from the residual
    // between published prices and the model's in-sample predictions. Reuse the
    // already-built training rows rather than rebuilding feature vectors — keeps
    // the request path off a second O(rows × scans) pass.
    const inSamplePrices = new Map<string, number>();
    const inSamplePredicted = new Map<string, number>();
    for (let i = 0; i < training.keys.length; i++) {
      const key = training.keys[i];
      const row = training.features.rows[i];
      const target = training.targets[i];
      if (key === undefined || row === undefined || target === undefined) {
        continue;
      }
      inSamplePrices.set(key, target);
      inSamplePredicted.set(key, fitted.predict(row));
    }
    const { biasByHour, globalBias } = fitHourBias(
      inSamplePrices,
      inSamplePredicted,
    );
    predicted = applyHourBias(predicted, biasByHour, globalBias);
    hourBiasBuckets = biasByHour.size;
  }

  let floorClippedQuarters = 0;
  if (floor !== null) {
    const clipped = new Map<string, number>();
    for (const [key, value] of predicted) {
      if (value < floor) {
        clipped.set(key, floor);
        floorClippedQuarters++;
      } else {
        clipped.set(key, value);
      }
    }
    predicted = clipped;
  }

  const built = buildPredictedSeries(predicted, seriesStartMs, numQuarters);

  const diagnostics: ForecastDiagnostics = {
    fitSamples: fitted.meta.sampleCount,
    featureCount: fitted.meta.featureCount,
    fitUsedDefault,
    consumptionExtendedQuarters: consExtended.size - consQ.size,
    windExtendedQuarters: windExtended.size - windQ.size,
    filledQuarters: built.filledQuarters,
    zeroSeededQuarters: built.zeroSeededQuarters,
    hourBiasBuckets,
    predictionFloor: floor,
    floorClippedQuarters,
  };

  return { series: built.series, diagnostics };
};

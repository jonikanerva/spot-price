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
import { applyBand, type CalibratedBands } from "./conformal.js";
import { CALIBRATED_BANDS } from "./conformal-artifact.js";

/**
 * Pure FI price-forecast pipeline. A pluggable closed-form regression over a
 * rich feature set (grid residual + wind/consumption + an interaction term, FI
 * and neighbour price lags, UTC calendar) — still no machine learning: the
 * default `Model` is hand-rolled ridge (`model.ts`), and an optional price floor
 * refines the level afterwards.
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
// Within-day shape decomposition
// ---------------------------------------------------------------------------

/**
 * Look up the persistence value for a predicted quarter at `ms`: the published
 * spot price one day earlier, else one week earlier, else null. UTC-only lag-key
 * construction identical to `features.ts` (`ms − DAY_MS` / `ms − WEEK_MS` +
 * `quarterKey`), so the persistence shape stays exactly the strong weekly/daily
 * seasonality the model's own lag features draw on.
 */
const persistenceAt = (
  ms: number,
  spotPricesByKey: ReadonlyMap<string, number>,
): number | null => {
  const oneDay = spotPricesByKey.get(quarterKey(ms - DAY_MS));
  if (oneDay !== undefined) {
    return oneDay;
  }
  const oneWeek = spotPricesByKey.get(quarterKey(ms - WEEK_MS));
  if (oneWeek !== undefined) {
    return oneWeek;
  }
  return null;
};

/**
 * Decompose each predicted quarter into the ridge model's per-UTC-day LEVEL
 * (which has real skill — lower MAE, healthier band residuals) and a within-day
 * SHAPE taken from persistence (the 1d/7d lag profile that wins the within-day
 * ranking the ridge level is blind to). For each UTC day:
 *
 *   out(q) = ridgeDailyMean + (persistence(q) − persistenceDailyMean)
 *
 * where the daily means are over that day's quarters (`persistenceDailyMean`
 * only over quarters that HAVE a persistence value). Quarters with no 1d/7d lag
 * fall back to the flat `ridgeDailyMean` and increment `shapeFallbackQuarters`.
 *
 * WHY rank-identical to pure persistence within a day: every quarter on a UTC
 * day shares the same additive constant `(ridgeDailyMean − persistenceDailyMean)`,
 * so the within-day ORDER of the output equals the order of `persistence(q)`
 * exactly — the decomposition reaches persistence's within-day rank skill while
 * keeping ridge's level. It has ZERO tuned hyperparameters (no blend weight, no
 * grid search), so it cannot leak a tuned knob. Pure and UTC-only.
 *
 * The DoD's per-horizon rank metrics are computed within a single forecast day
 * and each backtest horizon bucket spans exactly one UTC day (`backtest.ts` keys
 * horizons off `floor((targetMs − originMs)/DAY_MS)` with `originMs` one quarter
 * after the last published delivery, i.e. UTC midnight), so the per-day constant
 * never changes Spearman / precision@N — the equivalence above holds by
 * construction.
 */
export const applyWithinDayShape = (
  predicted: ReadonlyMap<string, number>,
  spotPricesByKey: ReadonlyMap<string, number>,
): { result: Map<string, number>; shapeFallbackQuarters: number } => {
  // Group predicted quarters by UTC day and accumulate the ridge level mean plus
  // the persistence mean over quarters that carry a lag value.
  interface DayAgg {
    ridgeSum: number;
    ridgeCount: number;
    persistenceSum: number;
    persistenceCount: number;
  }
  const byDay = new Map<number, DayAgg>();
  for (const [key, value] of predicted) {
    const ms = new Date(key).getTime();
    if (!Number.isFinite(ms)) {
      continue;
    }
    const day = Math.floor(ms / DAY_MS) * DAY_MS;
    const agg = byDay.get(day) ?? {
      ridgeSum: 0,
      ridgeCount: 0,
      persistenceSum: 0,
      persistenceCount: 0,
    };
    agg.ridgeSum += value;
    agg.ridgeCount += 1;
    const persistence = persistenceAt(ms, spotPricesByKey);
    if (persistence !== null) {
      agg.persistenceSum += persistence;
      agg.persistenceCount += 1;
    }
    byDay.set(day, agg);
  }

  const result = new Map<string, number>();
  let shapeFallbackQuarters = 0;
  for (const [key, value] of predicted) {
    const ms = new Date(key).getTime();
    if (!Number.isFinite(ms)) {
      result.set(key, value);
      continue;
    }
    const day = Math.floor(ms / DAY_MS) * DAY_MS;
    const agg = byDay.get(day);
    if (agg === undefined || agg.ridgeCount === 0) {
      result.set(key, value);
      continue;
    }
    const ridgeDailyMean = agg.ridgeSum / agg.ridgeCount;
    const persistence = persistenceAt(ms, spotPricesByKey);
    if (persistence === null || agg.persistenceCount === 0) {
      // No lag for this quarter (or no lag anywhere on the day): flat level.
      result.set(key, ridgeDailyMean);
      shapeFallbackQuarters += 1;
      continue;
    }
    const persistenceDailyMean = agg.persistenceSum / agg.persistenceCount;
    result.set(key, ridgeDailyMean + (persistence - persistenceDailyMean));
  }

  return { result, shapeFallbackQuarters };
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
  /**
   * The empirical prediction-band artifact applied to the series. Defaults to
   * the committed `CALIBRATED_BANDS` (currently dark). Injectable so tests can
   * supply a calibrated stub. Bands are applied only to quarters carrying a real
   * prediction; forward-filled / zero-seeded quarters never get a band.
   */
  readonly bands?: CalibratedBands;
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
 * c/kWh) → `applyWithinDayShape` (ridge level + persistence within-day shape) →
 * price-floor clip → `buildPredictedSeries` → empirical band measured against the
 * post-shape, post-floor point. Stays pure — the model and feature builders take
 * no I/O.
 */
export const buildForecast = (
  input: ForecastInput,
  opts: ForecastOptions = {},
): ForecastResult => {
  const windWeeks = opts.windExtensionWeeks ?? WIND_EXTENSION_WEEKS;
  const consWeeks =
    opts.consumptionExtensionWeeks ?? CONSUMPTION_EXTENSION_WEEKS;
  const floor = opts.floor ?? null;
  const model = opts.model ?? createRidgeModel();
  const bands = opts.bands ?? CALIBRATED_BANDS;

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

  // Recover the within-day price RHYTHM the ridge level model is blind to by
  // stamping the persistence (1d/7d lag) shape onto the ridge per-day level.
  // Rank-identical to pure persistence within a UTC day (the rank skill) while
  // keeping ridge's level skill — no tuned hyperparameter (`applyWithinDayShape`).
  const shaped = applyWithinDayShape(predicted, input.spotPricesByKey);
  predicted = shaped.result;
  const shapeFallbackQuarters = shaped.shapeFallbackQuarters;

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

  // Empirical prediction band (P10/P90-style), applied as a pure arcsinh-space
  // lookup onto the FULL post-shape, post-floor point — the exact quantity the
  // offline backtest residual was measured against. Only quarters that carry a
  // REAL prediction get a band; a quarter the series forward-filled or
  // zero-seeded is absent from `predicted`, so it never receives bounds. When
  // the artifact is uncalibrated (`applyBand` → null), the series is unchanged.
  let bandHourBuckets = 0;
  const series: ForecastSpotPoint[] = built.series.map((point) => {
    if (!predicted.has(point.start)) {
      return point;
    }
    const utcHour = new Date(point.start).getUTCHours();
    const band = applyBand(point.estimatedSpotCentsKwh, utcHour, bands, floor);
    if (band === null) {
      return point;
    }
    bandHourBuckets++;
    return {
      ...point,
      estimatedSpotLowCentsKwh: band.low,
      estimatedSpotHighCentsKwh: band.high,
    };
  });

  const diagnostics: ForecastDiagnostics = {
    fitSamples: fitted.meta.sampleCount,
    featureCount: fitted.meta.featureCount,
    fitUsedDefault,
    consumptionExtendedQuarters: consExtended.size - consQ.size,
    windExtendedQuarters: windExtended.size - windQ.size,
    filledQuarters: built.filledQuarters,
    zeroSeededQuarters: built.zeroSeededQuarters,
    shapeFallbackQuarters,
    predictionFloor: floor,
    floorClippedQuarters,
    bandCalibrated: bands.calibrated,
    bandHourBuckets,
  };

  return { series, diagnostics };
};

import type { FeatureMatrix, FeatureVector } from "./model.js";
import { quarterKey } from "./forecast.js";

/**
 * PURE feature engineering for the FI price forecast.
 *
 * Strictly pure: no `pg`, no `fetch`, no `env`, no `Date.now()`, no mutable
 * module state. The I/O boundary (the route / the backtest harness) fetches the
 * data, buckets the Fingrid series to quarter keys, and assembles the neighbor
 * price maps; this module turns that already-aligned data into a design matrix
 * for training and a single feature vector per predicted quarter.
 *
 * All time arithmetic is UTC (`VISION.md → UTC internally`): inputs are keyed by
 * UTC ISO 8601 quarter keys (15-min floor), calendar features read `getUTCHours`
 * / `getUTCDay`, and there is NO local-time arithmetic below the response edge.
 *
 * The two entry points are deliberately separate so the caller cannot
 * accidentally reuse training rows as prediction inputs:
 *   - `buildTrainingMatrix` — over historical quarters that have a known target.
 *   - `buildFeatureVector`  — for one future quarter we want to predict.
 * Both share `assembleFeatures`, so a training row and the corresponding
 * prediction vector are guaranteed to have identical column semantics and order.
 */

const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const WEEK_MS = 7 * DAY_MS;

/**
 * Training history window: how far back from the prediction origin we draw
 * labelled quarters for the fit. Deliberately a NAMED, BOUNDED constant — the
 * feature builder must never silently depend on the unbounded `prices` table
 * for an open-ended window. Reuses the ~30-day single-season floor window so
 * the model and the price floor track the same recent slice of Finnish prices
 * (`forecast.ts → FLOOR_HISTORY_DAYS`, kept in sync). 30 days × 96 quarters ≈
 * 2 880 training rows.
 */
export const TRAINING_HISTORY_DAYS = 30;

/** Neighbor Nord Pool areas whose lagged prices inform the FI estimate. */
export const NEIGHBOR_AREAS: readonly string[] = ["SE1", "SE3", "EE"];

/**
 * Stable feature-column order. The training matrix and every prediction vector
 * use exactly this order; the column count is the contract every row obeys.
 */
export const FEATURE_NAMES: readonly string[] = [
  // Demand/supply residual (consumption - wind), the original driver.
  "residual_mw",
  "wind_mw",
  "consumption_mw",
  "wind_x_consumption", // interaction term (standardized inputs' product)
  // FI price lags.
  "fi_latest_published",
  "fi_same_quarter_1d",
  "fi_same_quarter_7d",
  "fi_prev_day_min",
  "fi_prev_day_max",
  "fi_prev_day_mean",
  // Neighbor price lags (1d / 7d), in NEIGHBOR_AREAS order.
  "se1_1d",
  "se1_7d",
  "se3_1d",
  "se3_7d",
  "ee_1d",
  "ee_7d",
  // Calendar (UTC), cyclical encodings.
  "hour_sin",
  "hour_cos",
  "dow_sin",
  "dow_cos",
  "is_weekend",
];

/**
 * Inputs shared by both entry points. Every series is keyed by UTC ISO quarter
 * key; the caller has already bucketed Fingrid records and extended the wind /
 * consumption tails beyond the published horizon (`forecast.ts` helpers).
 */
export interface FeatureContext {
  /** Published FI spot prices (c/kWh) by quarter key — also the training target source. */
  readonly fiPricesByKey: ReadonlyMap<string, number>;
  /** Neighbor-area spot prices (c/kWh) by area → (quarter key → price). Missing → neutral fill. */
  readonly neighborPricesByArea: ReadonlyMap<
    string,
    ReadonlyMap<string, number>
  >;
  /** Wind (MW) by quarter key — forecast horizon + tail-extended. */
  readonly windByKey: ReadonlyMap<string, number>;
  /** Consumption (MW) by quarter key — forecast horizon + tail-extended. */
  readonly consumptionByKey: ReadonlyMap<string, number>;
}

/** Per-day aggregate stats for the day preceding a quarter (FI prices). */
interface PrevDayStats {
  readonly min: number;
  readonly max: number;
  readonly mean: number;
}

/**
 * Precomputed lookups derived once from the FI price series, so per-quarter
 * feature assembly stays O(1) instead of re-scanning the day / walking back for
 * every row (this is what keeps `buildForecast` on the request path under the
 * STACK §4 budget). Pure — a function of `fiPricesByKey` only.
 */
interface PriceIndex {
  /** Per-UTC-day min/max/mean of FI prices, keyed by the day-start ms. */
  readonly statsByDayStart: ReadonlyMap<number, PrevDayStats>;
  /** Distinct FI quarter timestamps (ms), ascending — for the latest-published lag. */
  readonly sortedMs: readonly number[];
  /** Global mean FI price used as the neutral lag fill. */
  readonly neutral: number;
}

const buildPriceIndex = (
  fiPricesByKey: ReadonlyMap<string, number>,
): PriceIndex => {
  const perDay = new Map<
    number,
    { min: number; max: number; sum: number; n: number }
  >();
  const allMs: number[] = [];
  let total = 0;
  let count = 0;
  for (const [key, price] of fiPricesByKey) {
    const ms = new Date(key).getTime();
    if (!Number.isFinite(ms)) {
      continue;
    }
    allMs.push(ms);
    total += price;
    count++;
    const dayStart = Math.floor(ms / DAY_MS) * DAY_MS;
    const agg = perDay.get(dayStart);
    if (agg === undefined) {
      perDay.set(dayStart, { min: price, max: price, sum: price, n: 1 });
    } else {
      if (price < agg.min) agg.min = price;
      if (price > agg.max) agg.max = price;
      agg.sum += price;
      agg.n++;
    }
  }
  const statsByDayStart = new Map<number, PrevDayStats>();
  for (const [dayStart, agg] of perDay) {
    statsByDayStart.set(dayStart, {
      min: agg.min,
      max: agg.max,
      mean: agg.sum / agg.n,
    });
  }
  allMs.sort((a, b) => a - b);
  return {
    statsByDayStart,
    sortedMs: allMs,
    neutral: count > 0 ? total / count : 0,
  };
};

/** Stats for the UTC day before `ms`, from the precomputed index. */
const prevDayStats = (
  index: PriceIndex,
  ms: number,
  neutral: number,
): PrevDayStats => {
  const prevDayStart = Math.floor(ms / DAY_MS) * DAY_MS - DAY_MS;
  return (
    index.statsByDayStart.get(prevDayStart) ?? {
      min: neutral,
      max: neutral,
      mean: neutral,
    }
  );
};

/**
 * Most recent FI price at or before `ms`, or the neutral fallback. Binary
 * search over the precomputed sorted timestamps, then a direct map lookup.
 */
const latestPublishedAtOrBefore = (
  fiPricesByKey: ReadonlyMap<string, number>,
  index: PriceIndex,
  ms: number,
  neutral: number,
): number => {
  const arr = index.sortedMs;
  let lo = 0;
  let hi = arr.length - 1;
  let bestMs: number | null = null;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const v = arr[mid] ?? 0;
    if (v <= ms) {
      bestMs = v;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  if (bestMs === null) {
    return neutral;
  }
  return fiPricesByKey.get(quarterKey(bestMs)) ?? neutral;
};

/**
 * Assemble the feature vector for the quarter at `ms`. Shared by training and
 * prediction so column semantics/order are identical. Missing inputs use the
 * supplied neutral fill — never a hard error, never a new `degraded` trigger.
 */
const assembleFeatures = (
  ms: number,
  ctx: FeatureContext,
  index: PriceIndex,
  neutral: number,
): FeatureVector => {
  const key = quarterKey(ms);

  // Grid drivers (already tail-extended by the caller). Missing → 0 MW, the
  // natural neutral for a residual.
  const wind = ctx.windByKey.get(key) ?? 0;
  const consumption = ctx.consumptionByKey.get(key) ?? 0;
  const residual = consumption - wind;
  // Interaction term on raw MW; standardization in the model puts it on scale.
  const windXConsumption = wind * consumption;

  // FI price lags.
  const fiLatest = latestPublishedAtOrBefore(
    ctx.fiPricesByKey,
    index,
    ms,
    neutral,
  );
  const fiSameQ1d = ctx.fiPricesByKey.get(quarterKey(ms - DAY_MS)) ?? neutral;
  const fiSameQ7d = ctx.fiPricesByKey.get(quarterKey(ms - WEEK_MS)) ?? neutral;
  const prev = prevDayStats(index, ms, neutral);

  // Neighbor price lags (1d / 7d) in NEIGHBOR_AREAS order; neutral fill when an
  // area is entirely absent or that specific lag quarter is missing.
  const neighborLags: number[] = [];
  for (const area of NEIGHBOR_AREAS) {
    const series = ctx.neighborPricesByArea.get(area);
    const lag1d = series?.get(quarterKey(ms - DAY_MS)) ?? neutral;
    const lag7d = series?.get(quarterKey(ms - WEEK_MS)) ?? neutral;
    neighborLags.push(lag1d, lag7d);
  }

  // Calendar (UTC only). Cyclical sin/cos so 23:00→00:00 and Sun→Mon are
  // continuous; plus an explicit weekend flag (Finnish demand drops at weekends).
  const date = new Date(ms);
  const hour = date.getUTCHours();
  const dow = date.getUTCDay(); // 0 = Sunday
  const hourSin = Math.sin((2 * Math.PI * hour) / 24);
  const hourCos = Math.cos((2 * Math.PI * hour) / 24);
  const dowSin = Math.sin((2 * Math.PI * dow) / 7);
  const dowCos = Math.cos((2 * Math.PI * dow) / 7);
  const isWeekend = dow === 0 || dow === 6 ? 1 : 0;

  return [
    residual,
    wind,
    consumption,
    windXConsumption,
    fiLatest,
    fiSameQ1d,
    fiSameQ7d,
    prev.min,
    prev.max,
    prev.mean,
    ...neighborLags,
    hourSin,
    hourCos,
    dowSin,
    dowCos,
    isWeekend,
  ];
};

/**
 * Build a single prediction feature vector for the quarter at `targetMs`.
 * Column order/semantics match `buildTrainingMatrix` exactly.
 *
 * Builds the price index on each call. When predicting many quarters in a loop,
 * prefer `buildFeatureVectorIndexed` with a shared index to avoid rebuilding it
 * per quarter (the request path does this).
 */
export const buildFeatureVector = (
  targetMs: number,
  ctx: FeatureContext,
): FeatureVector => {
  const index = buildPriceIndex(ctx.fiPricesByKey);
  return assembleFeatures(targetMs, ctx, index, index.neutral);
};

/**
 * Like `buildFeatureVector` but reuses a precomputed `PriceIndex` so a batch of
 * predicted quarters shares the O(prices) index build instead of repeating it.
 * The index is built with `buildForecastPriceIndex`.
 */
export const buildFeatureVectorIndexed = (
  targetMs: number,
  ctx: FeatureContext,
  index: PriceIndex,
): FeatureVector => assembleFeatures(targetMs, ctx, index, index.neutral);

/** Precompute the shared price index for a forecast run. Pure. */
export const buildForecastPriceIndex = (ctx: FeatureContext): PriceIndex =>
  buildPriceIndex(ctx.fiPricesByKey);

export type { PriceIndex };

/** Aligned design matrix + targets for the closed-form fit. */
export interface BuiltTraining {
  readonly features: FeatureMatrix;
  readonly targets: readonly number[];
  /**
   * The UTC ISO quarter key for each row, in row order. Lets the caller reuse
   * the already-built training rows for in-sample work (e.g. the per-hour bias)
   * without rebuilding feature vectors — keeping the request path off a second
   * O(rows × scans) pass.
   */
  readonly keys: readonly string[];
}

/**
 * Build the training matrix over the FI quarters in
 * `[originMs - TRAINING_HISTORY_DAYS, originMs)` that have a published target.
 * `originMs` is the prediction origin (the issue time / one quarter after the
 * last published price). Only quarters with a known FI price become rows, so
 * every row has an aligned target.
 */
export const buildTrainingMatrix = (
  originMs: number,
  ctx: FeatureContext,
  sharedIndex?: PriceIndex,
): BuiltTraining => {
  const index = sharedIndex ?? buildPriceIndex(ctx.fiPricesByKey);
  const neutral = index.neutral;
  const windowStart = originMs - TRAINING_HISTORY_DAYS * DAY_MS;

  // Iterate the price keys (sorted) within the bounded window; each becomes a
  // row with its own target. This is what bounds the history — we never scan an
  // open-ended table.
  const priceKeys = [...ctx.fiPricesByKey.keys()].sort();
  const rows: FeatureVector[] = [];
  const targets: number[] = [];
  const keys: string[] = [];
  for (const key of priceKeys) {
    const ms = new Date(key).getTime();
    if (!Number.isFinite(ms) || ms < windowStart || ms >= originMs) {
      continue;
    }
    const target = ctx.fiPricesByKey.get(key);
    if (target === undefined) {
      continue;
    }
    rows.push(assembleFeatures(ms, ctx, index, neutral));
    targets.push(target);
    keys.push(key);
  }

  return {
    features: { featureNames: FEATURE_NAMES, rows },
    targets,
    keys,
  };
};

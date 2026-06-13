/**
 * OFFLINE / DEV-ONLY rolling-origin backtest ENGINE for the FI price forecast.
 *
 * This is a PURE LIBRARY: it has no `main` and no script self-guard. It lives in
 * `tools/` so it can never reach the production bundle (tsup's only entry is
 * `src/index.ts`, and the ESLint guard forbids `src/` runtime from importing
 * `tools/`). The single runnable entry point is `tools/backtest-cli.ts`; the
 * band-regeneration script is `tools/regenerate-bands.ts`. Both, and the tests,
 * import this engine. It adds NO background job (`STACK §9`).
 *
 * Why issue-time keyed (CORRECTNESS-CRITICAL): the forecast is only honestly
 * evaluable against the information the route ACTUALLY had when it issued the
 * forecast. So each rolling origin is a FORECAST-ISSUE TIME, and at that origin
 * we reconstruct exactly the route's information set:
 *   - FI (and neighbour) prices only through the last published delivery known
 *     at issue time;
 *   - Fingrid ACTUALS (datasets 75 / 124) only through issue time;
 *   - Fingrid FORECAST datasets (245 / 165) as the ONLY forward-looking input.
 * Scoring is against the realised (uncensored) prices. Metrics are absolute
 * (MAE / rMAE / sMAPE versus same-quarter-last-week and last-published-day
 * baselines) and rank-based (Spearman, precision@N), reported per horizon.
 *
 * Strictly pure: every export takes no I/O (no `process.env`, no network, no
 * DB). Fixture/DB loading lives in the CLI and regen script.
 */
import { readFileSync } from "node:fs";
import { buildForecast, quarterKey } from "../src/forecast.js";
import {
  buildArtifact,
  deriveBandOffsets,
  observedCoverageOf,
  type CalibratedBands,
  type HorizonResidual,
} from "../src/conformal.js";
import { precisionAtN, spearman } from "./backtest-metrics.js";
import type { FingridRecord } from "../src/types.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const FORECAST_DAYS = 3;

/** FI day-ahead publishes ~14:00 EET ≈ 12:00 UTC; prices for "tomorrow" appear then. */
const PUBLICATION_HOUR_UTC = 12;

/**
 * Horizon labels d+1/d+2/d+3 for forecast days 0/1/2 ahead of the series start.
 * Index 0 (the first forecast day, [origin, origin+24h)) is "d+1" — the day
 * AFTER the last published price, which is what an automation plans against.
 */
export const HORIZON_LABELS = ["d+1", "d+2", "d+3"] as const;
export type Horizon = (typeof HORIZON_LABELS)[number];

/**
 * precision@N window for the rank metric: the product-relevant flexible-load
 * window. N = 16 quarters = 4 hours — a typical sauna/EV/heating block. "Did the
 * forecast pick the genuinely cheapest (and peak) 4h of the day?" is the
 * product's actual use case, so this is the headline rank signal.
 */
export const PRECISION_N_QUARTERS = 16;

const msOf = (iso: string): number => new Date(iso).getTime();

// ---------------------------------------------------------------------------
// The information set knowable at a forecast-issue time
// ---------------------------------------------------------------------------

/** A spot price observation by area. */
export interface PricePoint {
  readonly area: string;
  readonly start: string;
  readonly spotCentsKwh: number;
}

/** Everything the backtest reads. Public grid + price data only — no user data. */
export interface BacktestData {
  readonly prices: readonly PricePoint[];
  /** Fingrid records grouped by dataset id. */
  readonly fingridByDataset: Readonly<Record<string, readonly FingridRecord[]>>;
}

/**
 * The censored slice of inputs available to the forecast at issue time `tMs`,
 * plus the price keys the model is allowed to learn lags from. This is the
 * single source of "what did we know at issue time" — both the model run and
 * the leakage guard read it, so they cannot drift apart.
 */
export interface IssueTimeInputs {
  readonly tMs: number;
  /** Last published FI delivery known at issue time (UTC ms), or null. */
  readonly lastPublishedMs: number | null;
  /**
   * The forecast series start (UTC ms): one quarter after the last published
   * delivery, or `tMs` when nothing is published yet. This is the boundary the
   * model is allowed to learn from — published day-ahead prices legitimately
   * carry delivery timestamps in the future relative to wall-clock `tMs` (a
   * forecast issued at 13:00 UTC already knows tomorrow's published prices), so
   * for PRICE knowledge "issue time" means this publication-derived origin, not
   * the wall clock. Everything the model lags/trains on must be strictly before
   * `originMs`; only the Fingrid forecast datasets may reach beyond it.
   */
  readonly originMs: number;
  /** FI spot (c/kWh) by quarter key, only through the last published delivery. */
  readonly fiPricesByKey: ReadonlyMap<string, number>;
  /** Neighbour spot (c/kWh) by area → quarter key, only through last published delivery. */
  readonly neighborPricesByArea: ReadonlyMap<
    string,
    ReadonlyMap<string, number>
  >;
  /** Fingrid forecast records (forward-looking input) by dataset. */
  readonly fingridForecast: Readonly<Record<string, readonly FingridRecord[]>>;
  /** Fingrid actual records (only through issue time) by dataset. */
  readonly fingridActual: Readonly<Record<string, readonly FingridRecord[]>>;
}

/**
 * The publication horizon: prices delivered before this instant are knowable at
 * issue time `tMs`. Before publication hour we know through today's end; at or
 * after it, through tomorrow's end. Mirrors the day-ahead publication cadence.
 */
const publicationHorizonMs = (tMs: number): number => {
  const todayEnd = Math.floor(tMs / DAY_MS) * DAY_MS + DAY_MS;
  const hour = new Date(tMs).getUTCHours();
  return hour >= PUBLICATION_HOUR_UTC ? todayEnd + DAY_MS : todayEnd;
};

/** Build the issue-time information set. Pure. */
export const reconstructIssueTime = (
  data: BacktestData,
  tMs: number,
): IssueTimeInputs => {
  const horizon = publicationHorizonMs(tMs);

  const fiPricesByKey = new Map<string, number>();
  const neighborPricesByArea = new Map<string, Map<string, number>>();
  let lastPublishedMs: number | null = null;

  for (const p of data.prices) {
    const startMs = msOf(p.start);
    if (!Number.isFinite(startMs) || startMs >= horizon) {
      continue;
    }
    const key = quarterKey(startMs);
    if (p.area === "FI") {
      fiPricesByKey.set(key, p.spotCentsKwh);
      if (lastPublishedMs === null || startMs > lastPublishedMs) {
        lastPublishedMs = startMs;
      }
    } else {
      const bucket =
        neighborPricesByArea.get(p.area) ?? new Map<string, number>();
      bucket.set(key, p.spotCentsKwh);
      neighborPricesByArea.set(p.area, bucket);
    }
  }

  // Fingrid forecast datasets are forward-looking and knowable at issue; actuals
  // are only known strictly before issue time.
  const fingridForecast: Record<string, FingridRecord[]> = {};
  const fingridActual: Record<string, FingridRecord[]> = {};
  const forecastDatasets = new Set(["245", "165"]);
  for (const [dataset, records] of Object.entries(data.fingridByDataset)) {
    if (forecastDatasets.has(dataset)) {
      fingridForecast[dataset] = [...records];
    } else {
      fingridActual[dataset] = records.filter((r) => msOf(r.startTime) < tMs);
    }
  }

  const originMs =
    lastPublishedMs !== null ? lastPublishedMs + QUARTER_MS : tMs;

  return {
    tMs,
    lastPublishedMs,
    originMs,
    fiPricesByKey,
    neighborPricesByArea,
    fingridForecast,
    fingridActual,
  };
};

// ---------------------------------------------------------------------------
// Leakage detection — exported so the guard test can assert it directly
// ---------------------------------------------------------------------------

/**
 * Every distinct input timestamp (ms) the issue-time set exposes to the model,
 * tagged with the boundary it must respect:
 *   - `boundaryMs` is the latest timestamp the source is allowed to reference;
 *   - `forwardLookingAllowed` marks the Fingrid forecast datasets, the ONLY
 *     forward-looking input (no upper bound).
 * Prices/neighbour lags are bounded by `originMs` (published day-ahead prices
 * legitimately reach past wall-clock `tMs` — see `IssueTimeInputs.originMs`);
 * Fingrid actuals are bounded by `tMs`.
 */
export interface InputTimestamp {
  readonly source: string;
  readonly ms: number;
  readonly boundaryMs: number;
  readonly forwardLookingAllowed: boolean;
}

export const collectInputTimestamps = (
  inputs: IssueTimeInputs,
): readonly InputTimestamp[] => {
  const out: InputTimestamp[] = [];
  for (const key of inputs.fiPricesByKey.keys()) {
    out.push({
      source: "fi_price",
      ms: msOf(key),
      boundaryMs: inputs.originMs,
      forwardLookingAllowed: false,
    });
  }
  for (const [area, series] of inputs.neighborPricesByArea) {
    for (const key of series.keys()) {
      out.push({
        source: `neighbor_${area}`,
        ms: msOf(key),
        boundaryMs: inputs.originMs,
        forwardLookingAllowed: false,
      });
    }
  }
  for (const [dataset, records] of Object.entries(inputs.fingridActual)) {
    for (const r of records) {
      out.push({
        source: `fingrid_actual_${dataset}`,
        ms: msOf(r.startTime),
        boundaryMs: inputs.tMs,
        forwardLookingAllowed: false,
      });
    }
  }
  for (const [dataset, records] of Object.entries(inputs.fingridForecast)) {
    for (const r of records) {
      out.push({
        source: `fingrid_forecast_${dataset}`,
        ms: msOf(r.startTime),
        boundaryMs: Number.POSITIVE_INFINITY,
        forwardLookingAllowed: true,
      });
    }
  }
  return out;
};

/**
 * The set of inputs that leak future information: a non-forward-looking source
 * referencing a timestamp at or after its allowed boundary. An empty result
 * means no leakage. The guard test asserts this is always empty.
 */
export const findLeakingInputs = (
  inputs: IssueTimeInputs,
): readonly InputTimestamp[] =>
  collectInputTimestamps(inputs).filter(
    (t) => !t.forwardLookingAllowed && t.ms >= t.boundaryMs,
  );

// ---------------------------------------------------------------------------
// Run the production forecast at one issue time
// ---------------------------------------------------------------------------

const ds = (
  byDataset: Readonly<Record<string, readonly FingridRecord[]>>,
  id: number,
): readonly FingridRecord[] => byDataset[String(id)] ?? [];

export interface IssueForecast {
  /** Predicted spot (c/kWh) by quarter key over the forecast horizon. */
  readonly predictedByKey: ReadonlyMap<string, number>;
  readonly seriesStartMs: number;
  readonly seriesEndMs: number;
  readonly usedFallback: boolean;
}

/** Run the production `buildForecast` against an issue-time information set. */
export const forecastAtIssueTime = (inputs: IssueTimeInputs): IssueForecast => {
  const seriesStartMs = inputs.originMs;
  const seriesEndMs = seriesStartMs + FORECAST_DAYS * DAY_MS;

  const result = buildForecast({
    spotPricesByKey: inputs.fiPricesByKey,
    neighborPricesByArea: inputs.neighborPricesByArea,
    windForecast: ds(inputs.fingridForecast, 245),
    windActual: ds(inputs.fingridActual, 75),
    consumptionForecast: ds(inputs.fingridForecast, 165),
    consumptionActual: ds(inputs.fingridActual, 124),
    seriesStartMs,
    seriesEndMs,
  });

  const predictedByKey = new Map<string, number>();
  for (const point of result.series) {
    predictedByKey.set(point.start, point.estimatedSpotCentsKwh);
  }
  return {
    predictedByKey,
    seriesStartMs,
    seriesEndMs,
    usedFallback: result.diagnostics.fitUsedDefault,
  };
};

// ---------------------------------------------------------------------------
// Metrics — MAE / rMAE / sMAPE
// ---------------------------------------------------------------------------

/** Aligned (predicted, actual) pairs. */
export type Pairs = readonly (readonly [number, number])[];

export const mae = (pairs: Pairs): number | null => {
  if (pairs.length === 0) {
    return null;
  }
  let sum = 0;
  for (const [p, a] of pairs) {
    sum += Math.abs(p - a);
  }
  return sum / pairs.length;
};

/** Symmetric MAPE in [0, 2]; pairs whose |p|+|a| is ~0 are skipped. */
export const smape = (pairs: Pairs): number | null => {
  let sum = 0;
  let n = 0;
  for (const [p, a] of pairs) {
    const denom = Math.abs(p) + Math.abs(a);
    if (denom < 1e-9) {
      continue;
    }
    sum += Math.abs(p - a) / denom;
    n++;
  }
  return n > 0 ? (2 * sum) / n : null;
};

/**
 * Relative MAE: model MAE divided by baseline MAE. < 1 means the model beats the
 * baseline. Null when the baseline MAE is undefined or zero.
 */
export const rmae = (
  modelMae: number | null,
  baselineMae: number | null,
): number | null => {
  if (modelMae === null || baselineMae === null || baselineMae === 0) {
    return null;
  }
  return modelMae / baselineMae;
};

// ---------------------------------------------------------------------------
// Naive baselines (knowable at issue time)
// ---------------------------------------------------------------------------

export type BaselineName = "last_week" | "last_published_day";

/** Baseline prediction for a target quarter, knowable at issue time. */
const baselineFor = (
  name: BaselineName,
  targetMs: number,
  inputs: IssueTimeInputs,
): number | undefined => {
  if (name === "last_week") {
    return inputs.fiPricesByKey.get(quarterKey(targetMs - 7 * DAY_MS));
  }
  // last_published_day: same quarter one day before the last published delivery.
  if (inputs.lastPublishedMs === null) {
    return undefined;
  }
  const lastDayStart = Math.floor(inputs.lastPublishedMs / DAY_MS) * DAY_MS;
  const sameTimeOfDay = (targetMs - lastDayStart) % DAY_MS;
  const offset = ((sameTimeOfDay % DAY_MS) + DAY_MS) % DAY_MS;
  return inputs.fiPricesByKey.get(quarterKey(lastDayStart + offset));
};

// ---------------------------------------------------------------------------
// Rolling-origin evaluation
// ---------------------------------------------------------------------------

/**
 * Per-horizon metrics over the leak-free scored pairs at that horizon. Absolute
 * (mae/smape) plus rank (spearman, precision@N for cheap and peak windows) plus
 * the empirical band coverage at that horizon. `pairs` is the scored count.
 * Any metric is null when undefined for the bucket (e.g. constant series).
 */
export interface HorizonMetrics {
  readonly mae: number | null;
  readonly smape: number | null;
  readonly spearman: number | null;
  readonly precisionAtNCheap: number | null;
  readonly precisionAtNPeak: number | null;
  readonly bandCoverage: number | null;
  readonly pairs: number;
}

export interface BacktestSummary {
  readonly origins: number;
  readonly fallbackOrigins: number;
  readonly modelMae: number | null;
  readonly modelSmape: number | null;
  readonly baselineMae: Readonly<Record<BaselineName, number | null>>;
  readonly rMae: Readonly<Record<BaselineName, number | null>>;
  /** True if every reconstructed origin was leakage-free. */
  readonly leakFree: boolean;
  /**
   * Per-horizon (d+1/d+2/d+3) metrics — a PURE post-hoc partition of the SAME
   * leakage-guarded pairs by `floor((targetMs − originMs)/DAY_MS)`. No new
   * leakage: it only re-buckets pairs already scored. Rank metrics are computed
   * per origin (so a quarter is ranked against its own day) and averaged.
   */
  readonly byHorizon: Readonly<Record<Horizon, HorizonMetrics>>;
  /**
   * Out-of-sample (predicted, actual) residuals tagged by UTC hour, harvested
   * from the SAME leakage-guarded realised-vs-predicted pairs used for the error
   * metrics. This — not the in-sample hour-bias pass — is the honest input the
   * conformal band derivation must use (see `conformal.ts`). Forward-filled and
   * zero-seeded quarters carry no band downstream, but they are harmless here
   * because they are scored exactly as the route would emit them.
   */
  readonly residuals: readonly HorizonResidual[];
}

const BASELINES: readonly BaselineName[] = ["last_week", "last_published_day"];

/**
 * Roll a forecast issue origin across each local day at the publication hour,
 * score it against realised prices, and aggregate. Pure — `data` is supplied by
 * the caller (the script reads it from fixtures).
 */
export const runBacktest = (data: BacktestData): BacktestSummary => {
  const realized = new Map<string, number>();
  let lo = Infinity;
  let hi = -Infinity;
  for (const p of data.prices) {
    if (p.area !== "FI") {
      continue;
    }
    const startMs = msOf(p.start);
    if (!Number.isFinite(startMs)) {
      continue;
    }
    realized.set(quarterKey(startMs), p.spotCentsKwh);
    if (startMs < lo) lo = startMs;
    if (startMs > hi) hi = startMs;
  }

  const modelPairs: [number, number][] = [];
  const residuals: HorizonResidual[] = [];
  const baselinePairs: Record<BaselineName, [number, number][]> = {
    last_week: [],
    last_published_day: [],
  };

  // Per-horizon accumulators. `pairs`/`residuals` are flat (for mae/smape/band
  // coverage); `originSeries` keeps one (preds, acts) series per origin so rank
  // metrics rank a quarter against ITS OWN forecast day, then average.
  const horizonAcc: Record<
    Horizon,
    {
      pairs: [number, number][];
      residuals: HorizonResidual[];
      originSeries: { preds: number[]; acts: number[] }[];
    }
  > = {
    "d+1": { pairs: [], residuals: [], originSeries: [] },
    "d+2": { pairs: [], residuals: [], originSeries: [] },
    "d+3": { pairs: [], residuals: [], originSeries: [] },
  };

  const horizonOf = (targetMs: number, originMs: number): Horizon | null => {
    const idx = Math.floor((targetMs - originMs) / DAY_MS);
    return idx >= 0 && idx < HORIZON_LABELS.length
      ? (HORIZON_LABELS[idx] ?? null)
      : null;
  };

  let origins = 0;
  let fallbackOrigins = 0;
  let leakFree = true;

  if (Number.isFinite(lo) && Number.isFinite(hi)) {
    let day = Math.floor(lo / DAY_MS) * DAY_MS;
    while (day <= hi) {
      const tMs = day + PUBLICATION_HOUR_UTC * HOUR_MS + HOUR_MS;
      day += DAY_MS;

      const inputs = reconstructIssueTime(data, tMs);
      if (findLeakingInputs(inputs).length > 0) {
        leakFree = false;
      }

      const fc = forecastAtIssueTime(inputs);
      // Per-origin, per-horizon series for the rank metrics this origin.
      const originByHorizon: Record<Horizon, { preds: number[]; acts: number[] }> =
        {
          "d+1": { preds: [], acts: [] },
          "d+2": { preds: [], acts: [] },
          "d+3": { preds: [], acts: [] },
        };
      // Only score origins that actually have realised prices to compare to.
      let scored = false;
      for (const [key, pred] of fc.predictedByKey) {
        const actual = realized.get(key);
        if (actual === undefined) {
          continue;
        }
        scored = true;
        modelPairs.push([pred, actual]);
        // Harvest the out-of-sample residual (tagged by the target's UTC hour)
        // for the conformal band — the same leakage-guarded pair scored above.
        const targetMs = msOf(key);
        const residual: HorizonResidual = {
          utcHour: new Date(key).getUTCHours(),
          predictedRaw: pred,
          actualRaw: actual,
        };
        residuals.push(residual);
        // Pure post-hoc partition by horizon — no new leakage.
        const horizon = horizonOf(targetMs, fc.seriesStartMs);
        if (horizon !== null) {
          const acc = horizonAcc[horizon];
          acc.pairs.push([pred, actual]);
          acc.residuals.push(residual);
          originByHorizon[horizon].preds.push(pred);
          originByHorizon[horizon].acts.push(actual);
        }
        for (const name of BASELINES) {
          const b = baselineFor(name, targetMs, inputs);
          if (b !== undefined) {
            baselinePairs[name].push([b, actual]);
          }
        }
      }
      if (scored) {
        origins++;
        if (fc.usedFallback) {
          fallbackOrigins++;
        }
        for (const h of HORIZON_LABELS) {
          if (originByHorizon[h].preds.length > 0) {
            horizonAcc[h].originSeries.push(originByHorizon[h]);
          }
        }
      }
    }
  }

  const modelMae = mae(modelPairs);
  const baselineMae: Record<BaselineName, number | null> = {
    last_week: mae(baselinePairs.last_week),
    last_published_day: mae(baselinePairs.last_published_day),
  };
  const rMae: Record<BaselineName, number | null> = {
    last_week: rmae(modelMae, baselineMae.last_week),
    last_published_day: rmae(modelMae, baselineMae.last_published_day),
  };

  const byHorizon: Record<Horizon, HorizonMetrics> = {
    "d+1": computeHorizonMetrics(horizonAcc["d+1"]),
    "d+2": computeHorizonMetrics(horizonAcc["d+2"]),
    "d+3": computeHorizonMetrics(horizonAcc["d+3"]),
  };

  return {
    origins,
    fallbackOrigins,
    modelMae,
    modelSmape: smape(modelPairs),
    baselineMae,
    rMae,
    leakFree,
    byHorizon,
    residuals,
  };
};

/** Mean of the non-null values, or null when there are none. */
const meanOrNull = (xs: readonly (number | null)[]): number | null => {
  const vals = xs.filter((x): x is number => x !== null);
  if (vals.length === 0) {
    return null;
  }
  return vals.reduce((a, b) => a + b, 0) / vals.length;
};

/**
 * Compute one horizon's metrics. Absolute (mae/smape) and band coverage run on
 * the flat pairs/residuals; rank metrics (spearman, precision@N) are computed
 * per origin and averaged so each quarter is ranked within its own forecast day.
 */
const computeHorizonMetrics = (acc: {
  pairs: [number, number][];
  residuals: HorizonResidual[];
  originSeries: { preds: number[]; acts: number[] }[];
}): HorizonMetrics => {
  const { offsetsByHour, globalOffsets } = deriveBandOffsets(acc.residuals);
  const bandCoverage =
    acc.residuals.length > 0
      ? observedCoverageOf(acc.residuals, offsetsByHour, globalOffsets)
      : null;

  const spearmans: (number | null)[] = [];
  const cheaps: (number | null)[] = [];
  const peaks: (number | null)[] = [];
  for (const s of acc.originSeries) {
    if (s.preds.length < 2) {
      continue;
    }
    spearmans.push(spearman(s.preds, s.acts));
    cheaps.push(precisionAtN(s.preds, s.acts, PRECISION_N_QUARTERS, "cheap"));
    peaks.push(precisionAtN(s.preds, s.acts, PRECISION_N_QUARTERS, "peak"));
  }

  return {
    mae: mae(acc.pairs),
    smape: smape(acc.pairs),
    spearman: meanOrNull(spearmans),
    precisionAtNCheap: meanOrNull(cheaps),
    precisionAtNPeak: meanOrNull(peaks),
    bandCoverage,
    pairs: acc.pairs.length,
  };
};

// ---------------------------------------------------------------------------
// Conformal band derivation from the backtest (offline)
// ---------------------------------------------------------------------------

/**
 * Run the backtest and build the committed band artifact from its out-of-sample
 * residuals. Pure — the offline regeneration script and the dev report both use
 * this so the gating logic lives in exactly one place. `generatedAt` is supplied
 * by the caller (offline scripts may pass a real clock; this module never reads
 * one). The artifact ships dark (`calibrated: false`) unless coverage clears the
 * gate (`conformal.ts`).
 */
export const deriveBandsFromBacktest = (
  data: BacktestData,
  generatedAt: string,
): { readonly summary: BacktestSummary; readonly bands: CalibratedBands } => {
  const summary = runBacktest(data);
  const bands = buildArtifact(summary.residuals, generatedAt);
  return { summary, bands };
};

// ---------------------------------------------------------------------------
// Fixture loading + script entry (dev only)
// ---------------------------------------------------------------------------

interface RawFingrid {
  readonly startTime: string;
  readonly endTime: string;
  readonly value: number;
}

/** Fixture price row; `area` is optional (older fixtures omit it → FI). */
interface RawPrice {
  readonly area?: string;
  readonly start: string;
  readonly spotCentsKwh: number;
}

interface Fixture {
  readonly prices: readonly RawPrice[];
  readonly fingrid: Readonly<Record<string, readonly RawFingrid[]>>;
}

const toRecords = (
  raw: readonly RawFingrid[],
  datasetId: number,
): FingridRecord[] =>
  raw.map((r) => ({
    datasetId,
    startTime: r.startTime,
    endTime: r.endTime,
    value: r.value,
  }));

/**
 * Read a backtest fixture from a single JSON FILE path (e.g.
 * `tools/backtest-data/fixture.json`). Symmetric with the CLI's `--export
 * <file>`: a `--db --export X.json` snapshot round-trips through `--data X.json`
 * with no re-conversion (the exported `spotCentsKwh` is already post-conversion).
 */
export const loadFixture = (filePath: string): BacktestData => {
  const text = readFileSync(filePath, "utf-8");
  const fixture = JSON.parse(text) as Fixture;
  // Tolerate the older tools/ fixture shape: prices without an `area` are FI.
  const prices: PricePoint[] = fixture.prices.map((p) => ({
    area: p.area ?? "FI",
    start: p.start,
    spotCentsKwh: p.spotCentsKwh,
  }));
  const fingridByDataset: Record<string, FingridRecord[]> = {};
  for (const [dataset, raw] of Object.entries(fixture.fingrid)) {
    fingridByDataset[dataset] = toRecords(raw, Number(dataset));
  }
  return { prices, fingridByDataset };
};

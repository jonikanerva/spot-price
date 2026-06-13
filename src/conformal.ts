/**
 * Calibrated EMPIRICAL prediction bands (P10 / P50 / P90) for the FI forecast.
 *
 * Strictly pure: no `pg`, no `fetch`, no `env`, no `Date.now()`, no mutable
 * module state. The band offsets are DERIVED OFFLINE from the issue-time
 * rolling-origin backtest (`backtest.ts`) — out-of-sample, leakage-guarded —
 * and shipped as a committed data artifact (`conformal-artifact.ts`). At request
 * time `applyBand` is a pure arcsinh-space lookup added onto the point estimate.
 *
 * ---------------------------------------------------------------------------
 * WHY OFFLINE / OUT-OF-SAMPLE RESIDUALS (load-bearing — do not violate).
 * ---------------------------------------------------------------------------
 * The band spread MUST come from the offline backtest, NOT from the in-sample
 * hour-bias pass in `forecast.ts`. After `applyHourBias`, the in-sample residual
 * is mean-zero per UTC hour BY CONSTRUCTION and is measured on the very rows the
 * model trained on — so harvesting band residuals there would be structurally
 * too tight, especially at spike hours, and dishonest. The honest spread is the
 * realised-vs-predicted error the backtest sees on data the forecast had never
 * seen at issue time.
 *
 * This is an EMPIRICAL band: a percentile of recent residual spread, NOT a
 * guaranteed coverage probability. Real prices can and do fall outside it,
 * especially at spikes. It ships only when measured `observedCoverage` clears
 * `COVERAGE_GATE_THRESHOLD`; otherwise it ships dark (`calibrated: false`).
 *
 * All residual arithmetic is in ARCSINH SPACE (the same transform the model fits
 * in), so the band is symmetric-in-transform and handles negative prices and
 * large spikes; offsets are `sinh`-inverted back to raw c/kWh at apply time.
 */

// ---------------------------------------------------------------------------
// Constants — the ship gate (binding decision, see PR description)
// ---------------------------------------------------------------------------

/**
 * Minimum measured out-of-sample coverage for the band to ship. Below this the
 * artifact is marked `calibrated: false` and the route emits no bound fields.
 * Set below the nominal target: an empirical band that lands inside its nominal
 * range ≥70% of the time is honest enough to surface (clearly labelled), while
 * a band covering <70% would mislead an automation and must stay dark.
 */
export const COVERAGE_GATE_THRESHOLD = 0.7;

/** Nominal coverage the P10/P90 offsets target (the 10th/90th residual pctl). */
export const NOMINAL_COVERAGE = 0.8;

/** Min residual samples for a per-UTC-hour offset; thinner hours pool to global. */
export const MIN_HOUR_CALIBRATION_SAMPLES = 14;

/** Min total residual samples before any calibration is attempted at all. */
export const MIN_TOTAL_CALIBRATION_SAMPLES = 96;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * Additive band offsets in ARCSINH SPACE for one UTC hour. `lowOffset ≤ 0`
 * (subtracted region below the point) and `highOffset ≥ 0` (above). Applied as
 * `sinh(asinh(point) + offset)`.
 */
export interface BandOffsets {
  readonly lowOffset: number;
  readonly highOffset: number;
}

/** One out-of-sample residual observation from the backtest, in RAW c/kWh. */
export interface HorizonResidual {
  readonly utcHour: number;
  readonly predictedRaw: number;
  readonly actualRaw: number;
}

/**
 * The committed band artifact. `calibrated` is the single switch the request
 * path reads: when false (default shipped state, or coverage below the gate),
 * no bands are emitted. Orthogonal to the forecast's `degraded`/`confidence`.
 */
export interface CalibratedBands {
  readonly method: "empirical-residual";
  readonly nominalCoverage: number;
  /** Measured out-of-sample coverage of the derived offsets, or null when uncalibrated. */
  readonly observedCoverage: number | null;
  readonly calibrated: boolean;
  /** Per-UTC-hour offsets (0-23). Empty when uncalibrated. */
  readonly offsetsByHour: ReadonlyMap<number, BandOffsets>;
  /** Pooled offsets used for hours below `MIN_HOUR_CALIBRATION_SAMPLES`, or null. */
  readonly globalOffsets: BandOffsets | null;
  /** ISO 8601 timestamp of artifact generation, or "" when uncalibrated. */
  readonly generatedAt: string;
}

export interface DeriveOptions {
  readonly nominalCoverage?: number;
  readonly minHourSamples?: number;
}

export interface DerivedOffsets {
  readonly offsetsByHour: ReadonlyMap<number, BandOffsets>;
  readonly globalOffsets: BandOffsets | null;
}

// ---------------------------------------------------------------------------
// arcsinh helpers
// ---------------------------------------------------------------------------

const asinh = (x: number): number => Math.asinh(x);
const sinh = (x: number): number => Math.sinh(x);

/**
 * Nearest-rank percentile of a sorted-ascending array. `p` in [0, 1].
 * Returns null for an empty array. Nearest-rank avoids interpolation artefacts
 * on small residual samples.
 */
const nearestRankPercentile = (
  sortedAsc: readonly number[],
  p: number,
): number | null => {
  const n = sortedAsc.length;
  if (n === 0) {
    return null;
  }
  // rank = ceil(p * n), clamped to [1, n]; index = rank - 1.
  const rank = Math.min(n, Math.max(1, Math.ceil(p * n)));
  return sortedAsc[rank - 1] ?? null;
};

/** arcsinh-space residual for a single pair: r = asinh(actual) − asinh(predicted). */
const residualArcsinh = (predictedRaw: number, actualRaw: number): number =>
  asinh(actualRaw) - asinh(predictedRaw);

/**
 * Offsets for one bucket of arcsinh residuals: lowOffset = the
 * (1−nominal)/2 percentile (≤ 0), highOffset = the (1+nominal)/2 percentile
 * (≥ 0). Returns null when the bucket is empty. The offsets are clamped so the
 * band can never invert (low ≤ 0 ≤ high) even on a degenerate one-sided sample.
 */
const offsetsFromResiduals = (
  residualsArcsinh: readonly number[],
  nominalCoverage: number,
): BandOffsets | null => {
  if (residualsArcsinh.length === 0) {
    return null;
  }
  const sorted = [...residualsArcsinh].sort((a, b) => a - b);
  const lowP = (1 - nominalCoverage) / 2;
  const highP = (1 + nominalCoverage) / 2;
  const low = nearestRankPercentile(sorted, lowP) ?? 0;
  const high = nearestRankPercentile(sorted, highP) ?? 0;
  return { lowOffset: Math.min(0, low), highOffset: Math.max(0, high) };
};

// ---------------------------------------------------------------------------
// Derivation (offline)
// ---------------------------------------------------------------------------

/**
 * Derive per-UTC-hour band offsets from out-of-sample backtest residuals, plus
 * a pooled global fallback for hours with too few samples. Pure.
 */
export const deriveBandOffsets = (
  residuals: readonly HorizonResidual[],
  opts: DeriveOptions = {},
): DerivedOffsets => {
  const nominal = opts.nominalCoverage ?? NOMINAL_COVERAGE;
  const minHour = opts.minHourSamples ?? MIN_HOUR_CALIBRATION_SAMPLES;

  const byHour = new Map<number, number[]>();
  const all: number[] = [];
  for (const r of residuals) {
    const res = residualArcsinh(r.predictedRaw, r.actualRaw);
    if (!Number.isFinite(res)) {
      continue;
    }
    const bucket = byHour.get(r.utcHour) ?? [];
    bucket.push(res);
    byHour.set(r.utcHour, bucket);
    all.push(res);
  }

  const globalOffsets = offsetsFromResiduals(all, nominal);

  const offsetsByHour = new Map<number, BandOffsets>();
  for (const [hour, bucket] of byHour) {
    if (bucket.length < minHour) {
      continue; // too thin → pool to global at apply time
    }
    const off = offsetsFromResiduals(bucket, nominal);
    if (off !== null) {
      offsetsByHour.set(hour, off);
    }
  }

  return { offsetsByHour, globalOffsets };
};

// ---------------------------------------------------------------------------
// Apply (request path) — pure lookup
// ---------------------------------------------------------------------------

/** The offsets to use for a UTC hour: the hour's own, else the pooled global. */
const offsetsForHour = (
  utcHour: number,
  bands: CalibratedBands,
): BandOffsets | null =>
  bands.offsetsByHour.get(utcHour) ?? bands.globalOffsets;

/**
 * Apply the band to a point estimate, returning `{ low, high }` in raw c/kWh, or
 * null when the bands are uncalibrated or no offsets exist for the hour.
 *
 * `pointRaw` MUST be the FULL post-bias, post-floor point estimate — that is
 * exactly what the backtest residual was measured against, so the artifact and
 * the application point stay semantically aligned. The lower bound is clipped
 * with the same price `floor` the point uses (an empirical band must not dip
 * below the floor the point estimate respects), and the result is forced to
 * `low ≤ point ≤ high` so the ordering invariant always holds.
 */
export const applyBand = (
  pointRaw: number,
  utcHour: number,
  bands: CalibratedBands,
  floor: number | null,
): { readonly low: number; readonly high: number } | null => {
  if (!bands.calibrated) {
    return null;
  }
  const off = offsetsForHour(utcHour, bands);
  if (off === null) {
    return null;
  }
  const pointA = asinh(pointRaw);
  let low = sinh(pointA + off.lowOffset);
  let high = sinh(pointA + off.highOffset);
  if (floor !== null && low < floor) {
    low = floor;
  }
  // Ordering invariant: low ≤ point ≤ high (floor-clip or rounding can't break it).
  low = Math.min(low, pointRaw);
  high = Math.max(high, pointRaw);
  return {
    low: Math.round(low * 1000) / 1000,
    high: Math.round(high * 1000) / 1000,
  };
};

// ---------------------------------------------------------------------------
// Observed coverage (offline gate input)
// ---------------------------------------------------------------------------

/**
 * Fraction of residuals whose realised price falls within the derived band for
 * its hour. The honest, out-of-sample measure that the ship gate checks. An
 * optional `floorByQuarter` is not keyed here (residuals carry no quarter key);
 * the floor only tightens the lower bound, so coverage computed without it is a
 * conservative (never-optimistic) estimate of the shipped band's coverage.
 */
export const observedCoverageOf = (
  residuals: readonly HorizonResidual[],
  offsetsByHour: ReadonlyMap<number, BandOffsets>,
  globalOffsets: BandOffsets | null,
): number => {
  if (residuals.length === 0) {
    return 0;
  }
  let covered = 0;
  let scored = 0;
  for (const r of residuals) {
    const off = offsetsByHour.get(r.utcHour) ?? globalOffsets;
    if (off === null) {
      continue;
    }
    scored++;
    const pointA = asinh(r.predictedRaw);
    const low = sinh(pointA + off.lowOffset);
    const high = sinh(pointA + off.highOffset);
    if (r.actualRaw >= low && r.actualRaw <= high) {
      covered++;
    }
  }
  return scored === 0 ? 0 : covered / scored;
};

// ---------------------------------------------------------------------------
// Artifact assembly (offline)
// ---------------------------------------------------------------------------

/**
 * Build the committed artifact from out-of-sample residuals. Sets
 * `calibrated: false` (and leaves the maps empty) when there are too few total
 * samples or measured coverage is below the ship gate — the honest dark state.
 */
export const buildArtifact = (
  residuals: readonly HorizonResidual[],
  generatedAt: string,
): CalibratedBands => {
  const uncalibrated: CalibratedBands = {
    method: "empirical-residual",
    nominalCoverage: NOMINAL_COVERAGE,
    observedCoverage: null,
    calibrated: false,
    offsetsByHour: new Map(),
    globalOffsets: null,
    generatedAt: "",
  };

  if (residuals.length < MIN_TOTAL_CALIBRATION_SAMPLES) {
    return uncalibrated;
  }

  const { offsetsByHour, globalOffsets } = deriveBandOffsets(residuals);
  if (globalOffsets === null && offsetsByHour.size === 0) {
    return uncalibrated;
  }

  const observedCoverage = observedCoverageOf(
    residuals,
    offsetsByHour,
    globalOffsets,
  );
  if (observedCoverage < COVERAGE_GATE_THRESHOLD) {
    // Coverage measured but below the gate — keep the number for transparency
    // but ship dark (no offsets emitted, calibrated: false).
    return { ...uncalibrated, observedCoverage };
  }

  return {
    method: "empirical-residual",
    nominalCoverage: NOMINAL_COVERAGE,
    observedCoverage,
    calibrated: true,
    offsetsByHour,
    globalOffsets,
    generatedAt,
  };
};

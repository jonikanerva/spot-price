/**
 * Pluggable, PURE, CLOSED-FORM regression model used by the FI price forecast.
 *
 * This module is strictly pure: no `pg`, no `fetch`, no `env`, no `Date.now()`,
 * no module-level mutable state. It preserves the `forecast.ts` purity
 * invariant — the I/O boundary (the route) supplies the data; this module fits
 * a model and predicts scalars.
 *
 * ---------------------------------------------------------------------------
 * STRUCTURAL PHASE-3 PIN — read before widening this interface.
 * ---------------------------------------------------------------------------
 * The `Model` interface below is deliberately narrow: it is for CLOSED-FORM,
 * SYNCHRONOUS, PURE scalar estimators only.
 *
 *   - `fit` returns SYNCHRONOUSLY (no Promise). A closed-form solver — ridge via
 *     the normal equations, OLS, ordinary least squares with basis expansion —
 *     finishes in a single call with bounded arithmetic; there is no training
 *     loop to await.
 *   - `predict` is a PURE scalar function `(FeatureVector) => number` with NO
 *     hidden state, NO I/O, NO mutation. Given the same fitted model and the
 *     same input it always returns the same number.
 *
 * Iterative optimizers (gradient descent / SGD), stateful estimators (online
 * learners, Kalman filters), tree ensembles (random forests, gradient-boosted
 * trees) and neural networks do NOT satisfy this interface. They need an
 * iteration budget, mutable fitted state, asynchronous training, or a
 * non-scalar prediction surface. Adopting any of them is a Phase-3 change that
 * requires BOTH (a) a wider interface here AND (b) a `VISION.md` change first —
 * the product's "simple, transparent math" principle (`VISION.md → Core
 * Principles` / `The forecast`) currently rules out learned non-linear models.
 *
 * This is NOT a generic plugin platform. There is exactly one implementation
 * (`createRidgeModel`). Do not add a second on-ramp implementation here.
 * ---------------------------------------------------------------------------
 */

/** A single row of features for one observation / one predicted quarter. */
export type FeatureVector = readonly number[];

/**
 * A design matrix: `featureNames` labels the columns (stable order), `rows` are
 * the per-observation feature vectors. Every row MUST have
 * `featureNames.length` entries (asserted by the feature builder's tests).
 */
export interface FeatureMatrix {
  readonly featureNames: readonly string[];
  readonly rows: readonly FeatureVector[];
}

/** Aligned design matrix + targets for fitting. `targets[i]` ↔ `features.rows[i]`. */
export interface TrainingData {
  readonly features: FeatureMatrix;
  readonly targets: readonly number[];
}

/** Provenance for the produced fit — drives the route's degraded/confidence signal. */
export interface FitMeta {
  /** True when the fit fell back to default coefficients (too few samples / singular system). */
  readonly usedFallback: boolean;
  /** Number of aligned training samples actually used. */
  readonly sampleCount: number;
  /** Number of feature columns. */
  readonly featureCount: number;
}

/** A fitted model: a pure scalar predictor plus fit provenance. */
export interface FittedModel {
  /** Pure: same input → same output, no state, no I/O. */
  readonly predict: (x: FeatureVector) => number;
  readonly meta: FitMeta;
}

/** Options for a single fit. */
export interface FitOptions {
  /** Ridge L2 penalty (λ). Defaults to a small positive value for stability. */
  readonly ridgeLambda?: number;
  /** Target transform applied before fitting; `predict` inverts it. */
  readonly priceTransform?: "arcsinh" | "none";
}

/**
 * The pluggable estimator. SEE THE PHASE-3 PIN ABOVE — closed-form / synchronous
 * / pure only.
 */
export interface Model {
  readonly fit: (data: TrainingData, opts?: FitOptions) => FittedModel;
}

// ---------------------------------------------------------------------------
// Tunable constants
// ---------------------------------------------------------------------------

/** Minimum aligned samples before we trust the fit; below this we fall back. */
export const MIN_FIT_SAMPLES = 24;
/** Default ridge penalty — small, keeps the system well-conditioned without over-shrinking. */
export const DEFAULT_RIDGE_LAMBDA = 1.0;
/**
 * Fallback prediction (in raw c/kWh) used when the fit is unusable. A neutral
 * mid-range Finnish spot price; the route degrades to low confidence in this
 * case so the value is never presented as a confident estimate.
 */
export const FALLBACK_PREDICTION = 3.0;

// ---------------------------------------------------------------------------
// Price transform (arcsinh) — symmetric, handles negatives and large spikes
// ---------------------------------------------------------------------------

/**
 * `asinh` is a signed log-like transform: near-linear for small values, log-like
 * for large magnitudes, and defined for negatives (spot prices can go negative).
 * Fitting in this space stops a few price spikes from dominating the least
 * squares, and `predict` inverts it with `sinh`.
 */
const asinh = (x: number): number => Math.asinh(x);
const sinh = (x: number): number => Math.sinh(x);

// ---------------------------------------------------------------------------
// Standardization (zero-mean / unit-variance per column)
// ---------------------------------------------------------------------------

interface Standardizer {
  readonly means: readonly number[];
  /** Per-column scale; a zero-variance column is given scale 1 (so it maps to 0). */
  readonly scales: readonly number[];
}

const computeStandardizer = (
  rows: readonly FeatureVector[],
  featureCount: number,
): Standardizer => {
  const means = new Array<number>(featureCount).fill(0);
  const scales = new Array<number>(featureCount).fill(1);
  const n = rows.length;
  if (n === 0) {
    return { means, scales };
  }
  for (let j = 0; j < featureCount; j++) {
    let sum = 0;
    for (const row of rows) {
      sum += row[j] ?? 0;
    }
    const mean = sum / n;
    means[j] = mean;
    let varSum = 0;
    for (const row of rows) {
      const d = (row[j] ?? 0) - mean;
      varSum += d * d;
    }
    const variance = varSum / n;
    const std = Math.sqrt(variance);
    // Zero-variance column → scale 1 so standardized value is exactly 0 and the
    // column contributes nothing rather than producing NaN.
    scales[j] = std > 1e-12 ? std : 1;
  }
  return { means, scales };
};

const standardizeRow = (
  row: FeatureVector,
  std: Standardizer,
  featureCount: number,
): number[] => {
  const out = new Array<number>(featureCount).fill(0);
  for (let j = 0; j < featureCount; j++) {
    const mean = std.means[j] ?? 0;
    const scale = std.scales[j] ?? 1;
    out[j] = ((row[j] ?? 0) - mean) / scale;
  }
  return out;
};

// ---------------------------------------------------------------------------
// Linear algebra (hand-rolled — no new dependency; ~40×40 is trivial)
// ---------------------------------------------------------------------------

/**
 * Solve a symmetric positive-definite system `A x = b` by Cholesky
 * decomposition (`A = L Lᵀ`), forward/back substitution. Returns `null` when
 * `A` is not numerically positive-definite (singular system) so the caller can
 * fall back. `A` is `n×n` row-major; `b` has length `n`.
 */
const solveSpd = (
  a: readonly (readonly number[])[],
  b: readonly number[],
  n: number,
): number[] | null => {
  // Cholesky: build lower-triangular L.
  const l: number[][] = Array.from({ length: n }, () =>
    new Array<number>(n).fill(0),
  );
  for (let i = 0; i < n; i++) {
    for (let j = 0; j <= i; j++) {
      let sum = a[i]?.[j] ?? 0;
      for (let k = 0; k < j; k++) {
        sum -= (l[i]?.[k] ?? 0) * (l[j]?.[k] ?? 0);
      }
      if (i === j) {
        if (sum <= 1e-12) {
          // Not positive-definite → singular / ill-conditioned.
          return null;
        }
        const lii = Math.sqrt(sum);
        const rowI = l[i];
        if (rowI) {
          rowI[j] = lii;
        }
      } else {
        const ljj = l[j]?.[j] ?? 0;
        if (ljj === 0) {
          return null;
        }
        const rowI = l[i];
        if (rowI) {
          rowI[j] = sum / ljj;
        }
      }
    }
  }
  // Forward substitution: L y = b.
  const y = new Array<number>(n).fill(0);
  for (let i = 0; i < n; i++) {
    let sum = b[i] ?? 0;
    for (let k = 0; k < i; k++) {
      sum -= (l[i]?.[k] ?? 0) * (y[k] ?? 0);
    }
    const lii = l[i]?.[i] ?? 0;
    if (lii === 0) {
      return null;
    }
    y[i] = sum / lii;
  }
  // Back substitution: Lᵀ x = y.
  const x = new Array<number>(n).fill(0);
  for (let i = n - 1; i >= 0; i--) {
    let sum = y[i] ?? 0;
    for (let k = i + 1; k < n; k++) {
      // Lᵀ[i][k] = L[k][i]
      sum -= (l[k]?.[i] ?? 0) * (x[k] ?? 0);
    }
    const lii = l[i]?.[i] ?? 0;
    if (lii === 0) {
      return null;
    }
    x[i] = sum / lii;
  }
  return x;
};

// ---------------------------------------------------------------------------
// Ridge regression (closed-form normal equations on standardized features)
// ---------------------------------------------------------------------------

/**
 * Build the augmented normal-equations system for ridge with an UNPENALIZED
 * intercept. Design columns are `[1, z_1, …, z_p]` where `z` are the
 * standardized features; the L2 penalty λ is added to the diagonal for the
 * feature columns only (index ≥ 1), never the intercept.
 *
 * Returns `(XᵀX + λI')` and `Xᵀy`, dimension `(p+1)`.
 */
const buildNormalEquations = (
  standardizedRows: readonly (readonly number[])[],
  targets: readonly number[],
  featureCount: number,
  lambda: number,
): { ata: number[][]; aty: number[] } => {
  const dim = featureCount + 1; // +1 intercept column
  const ata: number[][] = Array.from({ length: dim }, () =>
    new Array<number>(dim).fill(0),
  );
  const aty = new Array<number>(dim).fill(0);

  for (let r = 0; r < standardizedRows.length; r++) {
    const z = standardizedRows[r] ?? [];
    const target = targets[r] ?? 0;
    // Augmented row: x[0] = 1 (intercept), x[1+j] = z[j].
    const xrow = new Array<number>(dim).fill(0);
    xrow[0] = 1;
    for (let j = 0; j < featureCount; j++) {
      xrow[j + 1] = z[j] ?? 0;
    }
    for (let i = 0; i < dim; i++) {
      const xi = xrow[i] ?? 0;
      const rowI = ata[i];
      if (!rowI) {
        continue;
      }
      for (let k = 0; k < dim; k++) {
        rowI[k] = (rowI[k] ?? 0) + xi * (xrow[k] ?? 0);
      }
      aty[i] = (aty[i] ?? 0) + xi * target;
    }
  }
  // Ridge penalty on feature diagonal only (skip intercept at index 0).
  for (let i = 1; i < dim; i++) {
    const rowI = ata[i];
    if (rowI) {
      rowI[i] = (rowI[i] ?? 0) + lambda;
    }
  }
  return { ata, aty };
};

const fallbackFitted = (
  sampleCount: number,
  featureCount: number,
): FittedModel => ({
  predict: () => FALLBACK_PREDICTION,
  meta: { usedFallback: true, sampleCount, featureCount },
});

/**
 * Create the (only) closed-form ridge `Model`.
 *
 * Pipeline:
 *  1. Standardize features (zero-mean / unit-variance per column); means+scales
 *     are stored in the fitted model and re-applied in `predict`.
 *  2. Optionally transform the target with `arcsinh` (default) so spikes don't
 *     dominate the least squares.
 *  3. Solve the ridge normal equations `(XᵀX + λI)β = Xᵀy` with an unpenalized
 *     intercept, via hand-rolled Cholesky.
 *  4. `predict` standardizes the input, applies β, and `sinh`-inverts the
 *     target transform — returning raw c/kWh.
 *
 * Falls back to a constant prediction (`usedFallback: true`) when there are too
 * few aligned samples (< `MIN_FIT_SAMPLES`) or the system is singular.
 */
export const createRidgeModel = (): Model => ({
  fit: (data: TrainingData, opts: FitOptions = {}): FittedModel => {
    const lambda = opts.ridgeLambda ?? DEFAULT_RIDGE_LAMBDA;
    const transform = opts.priceTransform ?? "arcsinh";
    const featureCount = data.features.featureNames.length;
    const rows = data.features.rows;
    const sampleCount = Math.min(rows.length, data.targets.length);

    if (sampleCount < MIN_FIT_SAMPLES || featureCount === 0) {
      return fallbackFitted(sampleCount, featureCount);
    }

    const usableRows = rows.slice(0, sampleCount);
    const usableTargets = data.targets.slice(0, sampleCount);

    const std = computeStandardizer(usableRows, featureCount);
    const standardizedRows = usableRows.map((row) =>
      standardizeRow(row, std, featureCount),
    );
    const fitTargets =
      transform === "arcsinh" ? usableTargets.map(asinh) : usableTargets;

    const { ata, aty } = buildNormalEquations(
      standardizedRows,
      fitTargets,
      featureCount,
      lambda,
    );
    const beta = solveSpd(ata, aty, featureCount + 1);
    if (beta === null) {
      return fallbackFitted(sampleCount, featureCount);
    }

    const intercept = beta[0] ?? 0;
    // Capture coefficients/standardizer by value in the closure — predict stays pure.
    const coeffs = beta.slice(1);

    return {
      predict: (x: FeatureVector): number => {
        const z = standardizeRow(x, std, featureCount);
        let acc = intercept;
        for (let j = 0; j < featureCount; j++) {
          acc += (coeffs[j] ?? 0) * (z[j] ?? 0);
        }
        return transform === "arcsinh" ? sinh(acc) : acc;
      },
      meta: { usedFallback: false, sampleCount, featureCount },
    };
  },
});

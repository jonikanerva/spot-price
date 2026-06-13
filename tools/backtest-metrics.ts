/**
 * Dependency-free accuracy metrics for the forecast backtest harness.
 *
 * The forecast's primary use case is automation (picking the cheap/expensive
 * hours), so the rank-based metrics here are the primary signal; the
 * absolute-error metrics (mae/rmse/bias) are reference only. Every function
 * takes two equal-length arrays already aligned by the caller and returns a
 * number, or null where the metric is undefined (e.g. a constant series has no
 * rank correlation).
 *
 * Lives in tools/ (offline-only), imports nothing from src/, and never reaches
 * the production bundle. Reused by the backtest engine (`tools/backtest.ts`) for
 * the per-horizon rank metrics. Covered by `tools/backtest-metrics.test.ts`,
 * which runs via the tools test glob added to `vitest.config.ts`.
 */

const check = (pred: readonly number[], act: readonly number[]): void => {
  if (pred.length !== act.length) {
    throw new Error(
      `length mismatch: ${String(pred.length)} vs ${String(act.length)}`,
    );
  }
  if (pred.length === 0) {
    throw new Error("empty series");
  }
};

export const mae = (
  pred: readonly number[],
  act: readonly number[],
): number => {
  check(pred, act);
  let sum = 0;
  for (let i = 0; i < pred.length; i++) {
    sum += Math.abs((pred[i] ?? 0) - (act[i] ?? 0));
  }
  return sum / pred.length;
};

export const rmse = (
  pred: readonly number[],
  act: readonly number[],
): number => {
  check(pred, act);
  let sum = 0;
  for (let i = 0; i < pred.length; i++) {
    const d = (pred[i] ?? 0) - (act[i] ?? 0);
    sum += d * d;
  }
  return Math.sqrt(sum / pred.length);
};

export const bias = (
  pred: readonly number[],
  act: readonly number[],
): number => {
  check(pred, act);
  let sum = 0;
  for (let i = 0; i < pred.length; i++) {
    sum += (pred[i] ?? 0) - (act[i] ?? 0);
  }
  return sum / pred.length;
};

/** 1-based average ranks (ties resolved to the group's average rank). */
const averageRanks = (values: readonly number[]): number[] => {
  const n = values.length;
  const order = [...Array(n).keys()].sort(
    (a, b) => (values[a] ?? 0) - (values[b] ?? 0),
  );
  const ranks = new Array<number>(n).fill(0);
  let i = 0;
  while (i < n) {
    let j = i;
    while (j + 1 < n && values[order[j + 1] ?? 0] === values[order[i] ?? 0]) {
      j++;
    }
    const avgRank = (i + j) / 2 + 1;
    for (let k = i; k <= j; k++) {
      ranks[order[k] ?? 0] = avgRank;
    }
    i = j + 1;
  }
  return ranks;
};

/** Spearman rank correlation; null when either series is constant. */
export const spearman = (
  pred: readonly number[],
  act: readonly number[],
): number | null => {
  check(pred, act);
  const n = pred.length;
  if (n < 2) {
    return null;
  }
  const rp = averageRanks(pred);
  const ra = averageRanks(act);
  const mp = rp.reduce((a, b) => a + b, 0) / n;
  const ma = ra.reduce((a, b) => a + b, 0) / n;
  let cov = 0;
  let vp = 0;
  let va = 0;
  for (let i = 0; i < n; i++) {
    const dp = (rp[i] ?? 0) - mp;
    const da = (ra[i] ?? 0) - ma;
    cov += dp * da;
    vp += dp * dp;
    va += da * da;
  }
  if (vp === 0 || va === 0) {
    return null;
  }
  return cov / Math.sqrt(vp * va);
};

/**
 * Fraction of the predicted N-cheapest (or N-most-expensive) entries that are
 * genuinely in the actual N-cheapest (or N-most-expensive) set. The core
 * automation metric. Null for a constant prediction (no meaningful ranking).
 */
export const precisionAtN = (
  pred: readonly number[],
  act: readonly number[],
  n: number,
  mode: "cheap" | "peak",
): number | null => {
  check(pred, act);
  if (n <= 0) {
    throw new Error("n must be >= 1");
  }
  const total = pred.length;
  if (new Set(pred).size <= 1) {
    return null;
  }
  const take = Math.min(n, total);
  const reverse = mode === "peak";
  const indexBy = (arr: readonly number[]): number[] =>
    [...Array(total).keys()].sort((a, b) => {
      const diff = (arr[a] ?? 0) - (arr[b] ?? 0);
      const ordered = reverse ? -diff : diff;
      return ordered !== 0 ? ordered : a - b;
    });
  const predSet = new Set(indexBy(pred).slice(0, take));
  const actSet = new Set(indexBy(act).slice(0, take));
  let hits = 0;
  for (const idx of predSet) {
    if (actSet.has(idx)) {
      hits++;
    }
  }
  return hits / take;
};

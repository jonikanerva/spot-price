import { describe, expect, it } from "vitest";
import {
  bias,
  mae,
  median,
  percentile,
  precisionAtN,
  rms,
  rmse,
  spearman,
  standardDeviation,
} from "./backtest-metrics.js";

describe("backtest-metrics — absolute", () => {
  it("mae / rmse / bias on a known series", () => {
    const pred = [1, 2, 3];
    const act = [2, 2, 5];
    // errors: -1, 0, -2 → |.| mean = 1; sq mean = (1+0+4)/3; signed mean = -1.
    expect(mae(pred, act)).toBeCloseTo(1, 9);
    expect(rmse(pred, act)).toBeCloseTo(Math.sqrt(5 / 3), 9);
    expect(bias(pred, act)).toBeCloseTo(-1, 9);
  });

  it("throws on length mismatch and empty series", () => {
    expect(() => mae([1], [1, 2])).toThrow();
    expect(() => mae([], [])).toThrow();
  });
});

describe("spearman", () => {
  it("is 1 for a perfectly monotone-increasing relationship", () => {
    const pred = [1, 2, 3, 4, 5];
    const act = [10, 20, 30, 40, 50];
    expect(spearman(pred, act)).toBeCloseTo(1, 9);
  });

  it("is -1 for a perfectly reversed ranking", () => {
    const pred = [1, 2, 3, 4, 5];
    const act = [50, 40, 30, 20, 10];
    expect(spearman(pred, act)).toBeCloseTo(-1, 9);
  });

  it("is null for a constant series (no rank information)", () => {
    expect(spearman([3, 3, 3], [1, 2, 3])).toBeNull();
  });
});

describe("distribution summaries", () => {
  it("median handles odd, even, and empty", () => {
    expect(median([3, 1, 2])).toBe(2);
    expect(median([4, 1, 2, 3])).toBe(2.5);
    expect(median([])).toBeNull();
  });

  it("percentile interpolates and clamps to [0, 100]", () => {
    expect(percentile([0, 10], 50)).toBe(5);
    expect(percentile([0, 10, 20, 30], 90)).toBeCloseTo(27, 6);
    expect(percentile([5], 90)).toBe(5);
    expect(percentile([1, 2, 3], 200)).toBe(3); // clamped to 100th
    expect(percentile([], 90)).toBeNull();
  });

  it("standardDeviation is the sample (n−1) sd, null below 2 values", () => {
    // 2,4,6 → mean 4, var = (4+0+4)/2 = 4 → sd 2
    expect(standardDeviation([2, 4, 6])).toBeCloseTo(2, 6);
    expect(standardDeviation([7])).toBeNull();
    expect(standardDeviation([])).toBeNull();
  });

  it("rms is the quadratic mean, null on empty", () => {
    expect(rms([3, 4])).toBeCloseTo(Math.sqrt(12.5), 6);
    expect(rms([0, 0, 0])).toBe(0);
    expect(rms([])).toBeNull();
  });
});

describe("precisionAtN", () => {
  it("scores the overlap of the predicted vs actual cheapest-N set", () => {
    // actual cheapest-2 indices: values [5,1,9,2] → indices 1,3.
    // predicted [4,1,8,3] → cheapest-2 indices 1,3 → perfect overlap.
    expect(precisionAtN([4, 1, 8, 3], [5, 1, 9, 2], 2, "cheap")).toBeCloseTo(
      1,
      9,
    );
  });

  it("scores the peak-N window symmetrically", () => {
    // actual peak-2 indices for [5,1,9,2] → 2,0. predicted [4,1,8,3] peak-2 → 2,0.
    expect(precisionAtN([4, 1, 8, 3], [5, 1, 9, 2], 2, "peak")).toBeCloseTo(
      1,
      9,
    );
  });

  it("detects a wrong pick (partial overlap < 1)", () => {
    // actual cheapest-1 index for [5,1,9,2] → 1; predicted cheapest-1 for
    // [1,9,2,8] → 0. No overlap → 0.
    expect(precisionAtN([1, 9, 2, 8], [5, 1, 9, 2], 1, "cheap")).toBe(0);
  });

  it("is null for a constant prediction and throws on n <= 0", () => {
    expect(precisionAtN([2, 2, 2], [1, 2, 3], 1, "cheap")).toBeNull();
    expect(() => precisionAtN([1, 2], [1, 2], 0, "cheap")).toThrow();
  });
});

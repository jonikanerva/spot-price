import { describe, expect, it } from "vitest";
import {
  createRidgeModel,
  FALLBACK_PREDICTION,
  MIN_FIT_SAMPLES,
  type FeatureMatrix,
  type TrainingData,
} from "./model.js";

/**
 * Reference-checked unit tests for the closed-form ridge solver. A silent bug
 * in the linear algebra yields plausible-but-wrong prices, so every test pins
 * the result against an independently-derived expectation rather than just
 * asserting "something came back".
 */

const matrix = (
  names: readonly string[],
  rows: readonly (readonly number[])[],
): FeatureMatrix => ({ featureNames: names, rows });

const training = (
  names: readonly string[],
  rows: readonly (readonly number[])[],
  targets: readonly number[],
): TrainingData => ({ features: matrix(names, rows), targets });

describe("createRidgeModel — fallback behaviour", () => {
  it("falls back when there are fewer than MIN_FIT_SAMPLES samples", () => {
    const rows = Array.from({ length: MIN_FIT_SAMPLES - 1 }, (_unused, i) => [
      i,
    ]);
    const targets = rows.map((r) => (r[0] ?? 0) * 2);
    const model = createRidgeModel();
    const fitted = model.fit(training(["x"], rows, targets));
    expect(fitted.meta.usedFallback).toBe(true);
    expect(fitted.meta.sampleCount).toBe(MIN_FIT_SAMPLES - 1);
    expect(fitted.predict([10])).toBe(FALLBACK_PREDICTION);
  });

  it("falls back with zero features", () => {
    const rows = Array.from({ length: 50 }, () => []);
    const targets = rows.map(() => 1);
    const model = createRidgeModel();
    const fitted = model.fit(training([], rows, targets));
    expect(fitted.meta.usedFallback).toBe(true);
    expect(fitted.predict([])).toBe(FALLBACK_PREDICTION);
  });

  it("falls back on a singular (perfectly collinear) system with λ=0 and no transform", () => {
    // Two identical columns + λ=0 → XᵀX is singular → Cholesky must reject it.
    const rows = Array.from({ length: 40 }, (_unused, i) => [i, i]);
    const targets = rows.map((r) => (r[0] ?? 0) + 1);
    const model = createRidgeModel();
    const fitted = model.fit(training(["a", "b"], rows, targets), {
      ridgeLambda: 0,
      priceTransform: "none",
    });
    expect(fitted.meta.usedFallback).toBe(true);
  });
});

describe("createRidgeModel — λ=0 reduces to OLS", () => {
  it("recovers a known perfectly-linear target exactly (no transform)", () => {
    // y = 3*x1 - 2*x2 + 5, no noise. With λ=0 and no transform, ridge == OLS
    // and must recover the relationship to floating-point precision.
    const rows: number[][] = [];
    const targets: number[] = [];
    for (let i = 0; i < 60; i++) {
      const x1 = i;
      const x2 = (i * 7) % 13;
      rows.push([x1, x2]);
      targets.push(3 * x1 - 2 * x2 + 5);
    }
    const model = createRidgeModel();
    const fitted = model.fit(training(["x1", "x2"], rows, targets), {
      ridgeLambda: 0,
      priceTransform: "none",
    });
    expect(fitted.meta.usedFallback).toBe(false);
    // Predict at held-out points and compare to the exact formula.
    for (const [x1, x2] of [
      [100, 3],
      [-5, 11],
      [1000, 0],
    ] as const) {
      expect(fitted.predict([x1, x2])).toBeCloseTo(3 * x1 - 2 * x2 + 5, 6);
    }
  });

  it("matches a hand-computed single-feature OLS system", () => {
    // Hand system: points (x,y) = (1,3),(2,5),(3,7),(4,9) replicated to clear
    // MIN_FIT_SAMPLES. Exact OLS line is y = 2x + 1.
    const base: (readonly [number, number])[] = [
      [1, 3],
      [2, 5],
      [3, 7],
      [4, 9],
    ];
    const rows: number[][] = [];
    const targets: number[] = [];
    for (let rep = 0; rep < 10; rep++) {
      for (const [x, y] of base) {
        rows.push([x]);
        targets.push(y);
      }
    }
    const model = createRidgeModel();
    const fitted = model.fit(training(["x"], rows, targets), {
      ridgeLambda: 0,
      priceTransform: "none",
    });
    // y = 2x + 1 at several x values.
    expect(fitted.predict([0])).toBeCloseTo(1, 6);
    expect(fitted.predict([5])).toBeCloseTo(11, 6);
    expect(fitted.predict([10])).toBeCloseTo(21, 6);
  });
});

describe("createRidgeModel — ridge shrinkage", () => {
  it("shrinks coefficients toward zero as λ grows (prediction pulled to mean)", () => {
    const rows: number[][] = [];
    const targets: number[] = [];
    for (let i = 0; i < 60; i++) {
      rows.push([i]);
      targets.push(2 * i + 1);
    }
    const meanTarget = targets.reduce((a, b) => a + b, 0) / targets.length; // 60
    const model = createRidgeModel();
    const small = model.fit(training(["x"], rows, targets), {
      ridgeLambda: 0.001,
      priceTransform: "none",
    });
    const large = model.fit(training(["x"], rows, targets), {
      ridgeLambda: 1e6,
      priceTransform: "none",
    });
    // At a far-out point, heavy shrinkage pulls the prediction toward the mean
    // target (the intercept is unpenalized), so it is much closer to the mean
    // than the lightly-penalized fit.
    const far = [200];
    const smallPred = small.predict(far);
    const largePred = large.predict(far);
    expect(Math.abs(largePred - meanTarget)).toBeLessThan(
      Math.abs(smallPred - meanTarget),
    );
    expect(largePred).toBeCloseTo(meanTarget, 0);
  });
});

describe("createRidgeModel — arcsinh transform", () => {
  it("round-trips sinh(asinh(x)) ≈ x across magnitudes and signs", () => {
    for (const x of [-1234.5, -3.2, -0.01, 0, 0.01, 3.2, 1234.5]) {
      expect(Math.sinh(Math.asinh(x))).toBeCloseTo(x, 6);
    }
  });

  it("recovers a linear-in-arcsinh-space target through the transform", () => {
    // Construct targets so that asinh(y) = 0.5*x + 2 exactly; the fit (default
    // arcsinh transform) should recover y = sinh(0.5*x + 2) at predict time.
    const rows: number[][] = [];
    const targets: number[] = [];
    for (let i = 0; i < 60; i++) {
      const x = i / 10;
      rows.push([x]);
      targets.push(Math.sinh(0.5 * x + 2));
    }
    const model = createRidgeModel();
    const fitted = model.fit(training(["x"], rows, targets), {
      ridgeLambda: 0,
    });
    expect(fitted.meta.usedFallback).toBe(false);
    for (const x of [0, 2.5, 5]) {
      expect(fitted.predict([x])).toBeCloseTo(Math.sinh(0.5 * x + 2), 4);
    }
  });
});

describe("createRidgeModel — standardization correctness", () => {
  it("is invariant to feature rescaling/shifting (standardization cancels units)", () => {
    // The same underlying relationship expressed in different units must give
    // the same predictions, because features are standardized before the fit.
    const targets: number[] = [];
    const rawRows: number[][] = [];
    const scaledRows: number[][] = [];
    for (let i = 0; i < 60; i++) {
      const x = i;
      targets.push(4 * x + 7);
      rawRows.push([x]);
      // Same information, shifted + scaled units: x' = 1000 + 3*x.
      scaledRows.push([1000 + 3 * x]);
    }
    const model = createRidgeModel();
    const rawFit = model.fit(training(["x"], rawRows, targets), {
      ridgeLambda: 0,
      priceTransform: "none",
    });
    const scaledFit = model.fit(training(["x"], scaledRows, targets), {
      ridgeLambda: 0,
      priceTransform: "none",
    });
    // Predict at the corresponding point in each unit system.
    expect(rawFit.predict([30])).toBeCloseTo(scaledFit.predict([1000 + 90]), 4);
    // And both recover the true value y = 4*30 + 7 = 127.
    expect(rawFit.predict([30])).toBeCloseTo(127, 4);
  });

  it("ignores a zero-variance (constant) feature column without producing NaN", () => {
    const rows: number[][] = [];
    const targets: number[] = [];
    for (let i = 0; i < 60; i++) {
      rows.push([i, 42]); // second column constant
      targets.push(2 * i + 1);
    }
    const model = createRidgeModel();
    const fitted = model.fit(training(["x", "const"], rows, targets), {
      ridgeLambda: 0.0001,
      priceTransform: "none",
    });
    expect(fitted.meta.usedFallback).toBe(false);
    const p = fitted.predict([10, 42]);
    expect(Number.isFinite(p)).toBe(true);
    expect(p).toBeCloseTo(21, 0);
  });
});

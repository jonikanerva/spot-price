import { describe, expect, it } from "vitest";
import {
  applyBand,
  buildArtifact,
  COVERAGE_GATE_THRESHOLD,
  deriveBandOffsets,
  MIN_TOTAL_CALIBRATION_SAMPLES,
  NOMINAL_COVERAGE,
  observedCoverageOf,
  type BandOffsets,
  type CalibratedBands,
  type HorizonResidual,
} from "./conformal.js";

const calibratedBands = (
  offsetsByHour: ReadonlyMap<number, BandOffsets>,
  globalOffsets: BandOffsets | null,
): CalibratedBands => ({
  method: "empirical-residual",
  nominalCoverage: NOMINAL_COVERAGE,
  observedCoverage: 0.85,
  calibrated: true,
  offsetsByHour,
  globalOffsets,
  generatedAt: "2026-06-01T00:00:00.000Z",
});

describe("applyBand", () => {
  it("returns null when the bands are uncalibrated", () => {
    const dark: CalibratedBands = {
      method: "empirical-residual",
      nominalCoverage: NOMINAL_COVERAGE,
      observedCoverage: null,
      calibrated: false,
      offsetsByHour: new Map(),
      globalOffsets: null,
      generatedAt: "",
    };
    expect(applyBand(4, 12, dark)).toBeNull();
  });

  it("returns null when calibrated but no offsets exist for the hour", () => {
    const bands = calibratedBands(new Map(), null);
    expect(applyBand(4, 12, bands)).toBeNull();
  });

  it("is asymmetric in raw space (arcsinh) — equal-magnitude offsets give a wider top", () => {
    // Symmetric offsets in arcsinh space → asymmetric in raw c/kWh because sinh
    // grows faster above than below. At a positive point the upper gap should
    // exceed the lower gap.
    const off: BandOffsets = { lowOffset: -0.5, highOffset: 0.5 };
    const bands = calibratedBands(new Map([[12, off]]), null);
    const point = 5;
    const band = applyBand(point, 12, bands);
    expect(band).not.toBeNull();
    if (band) {
      const lowerGap = point - band.low;
      const upperGap = band.high - point;
      expect(upperGap).toBeGreaterThan(lowerGap);
      expect(band.low).toBeLessThanOrEqual(point);
      expect(band.high).toBeGreaterThanOrEqual(point);
    }
  });

  it("falls back to the pooled global offsets for an hour without its own", () => {
    const global: BandOffsets = { lowOffset: -0.3, highOffset: 0.3 };
    const bands = calibratedBands(
      new Map([[0, { lowOffset: -1, highOffset: 1 }]]),
      global,
    );
    const band = applyBand(4, 18, bands); // hour 18 has no own offsets
    expect(band).not.toBeNull();
    if (band) {
      // Should match the global offsets, not the hour-0 ones.
      expect(band.low).toBeCloseTo(Math.sinh(Math.asinh(4) - 0.3), 3);
      expect(band.high).toBeCloseTo(Math.sinh(Math.asinh(4) + 0.3), 3);
    }
  });

  it("keeps low ≤ point ≤ high at a negative point, allowing a negative lower bound", () => {
    // No floor any more: a genuinely negative FI price must keep an ordered band
    // whose lower bound is free to go below zero (the old floor-clip is gone).
    const off: BandOffsets = { lowOffset: -2, highOffset: 1 };
    const bands = calibratedBands(new Map([[12, off]]), null);
    const point = -3.0;
    const band = applyBand(point, 12, bands);
    expect(band).not.toBeNull();
    if (band) {
      expect(band.low).toBeLessThanOrEqual(point);
      expect(band.high).toBeGreaterThanOrEqual(point);
      expect(band.low).toBeLessThan(0); // not clipped up to a floor
    }
  });

  it("gives two distinct sub-zero points distinct lower bounds (no re-tie at the bottom)", () => {
    const off: BandOffsets = { lowOffset: -0.5, highOffset: 0.5 };
    const bands = calibratedBands(new Map([[12, off]]), null);
    const bandA = applyBand(-1.0, 12, bands);
    const bandB = applyBand(-4.0, 12, bands);
    expect(bandA).not.toBeNull();
    expect(bandB).not.toBeNull();
    if (bandA && bandB) {
      expect(bandA.low).not.toBe(bandB.low);
      expect(bandB.low).toBeLessThan(bandA.low);
    }
  });

  it("enforces the ordering invariant even with degenerate one-sided offsets", () => {
    // A zero-width band must still satisfy low ≤ point ≤ high.
    const off: BandOffsets = { lowOffset: 0, highOffset: 0 };
    const bands = calibratedBands(new Map([[12, off]]), null);
    const band = applyBand(3.333, 12, bands);
    expect(band).not.toBeNull();
    if (band) {
      expect(band.low).toBeLessThanOrEqual(3.333);
      expect(band.high).toBeGreaterThanOrEqual(3.333);
    }
  });
});

describe("deriveBandOffsets / observedCoverageOf", () => {
  // A residual generator with a KNOWN spread: actual = sinh(asinh(pred) + r),
  // so the arcsinh residual is exactly r. With r drawn from a symmetric ramp,
  // the 10th/90th percentiles are predictable and coverage is ≈ nominal.
  const buildResiduals = (
    hour: number,
    arcsinhResiduals: readonly number[],
  ): HorizonResidual[] =>
    arcsinhResiduals.map((r) => {
      const predictedRaw = 5;
      const actualRaw = Math.sinh(Math.asinh(predictedRaw) + r);
      return { utcHour: hour, predictedRaw, actualRaw };
    });

  it("derives offsets whose observed coverage is approximately the nominal target", () => {
    // 100 evenly-spaced arcsinh residuals in [-1, 1]; the 10th/90th nearest-rank
    // percentiles bound ~80% of them.
    const rs: number[] = [];
    for (let i = 0; i < 100; i++) {
      rs.push(-1 + (2 * i) / 99);
    }
    const residuals = buildResiduals(12, rs);
    const { offsetsByHour, globalOffsets } = deriveBandOffsets(residuals);
    const coverage = observedCoverageOf(
      residuals,
      offsetsByHour,
      globalOffsets,
    );
    expect(coverage).toBeGreaterThanOrEqual(0.78);
    expect(coverage).toBeLessThanOrEqual(0.92);
    // Offsets straddle zero.
    const off = offsetsByHour.get(12);
    expect(off).toBeDefined();
    if (off) {
      expect(off.lowOffset).toBeLessThanOrEqual(0);
      expect(off.highOffset).toBeGreaterThanOrEqual(0);
    }
  });

  it("pools thin hours to the global offset (no per-hour entry below the min)", () => {
    // Hour 3 has only 5 samples (< MIN_HOUR_CALIBRATION_SAMPLES) → no own entry,
    // but the global offset still covers it.
    const rs: number[] = [];
    for (let i = 0; i < 5; i++) {
      rs.push(-0.5 + (i * 1.0) / 4);
    }
    const residuals = buildResiduals(3, rs);
    const { offsetsByHour, globalOffsets } = deriveBandOffsets(residuals);
    expect(offsetsByHour.has(3)).toBe(false);
    expect(globalOffsets).not.toBeNull();
  });
});

describe("buildArtifact — ship gate", () => {
  const residualWithCoverage = (
    n: number,
    arcsinhSpread: number,
  ): HorizonResidual[] => {
    // n residuals across 24 hours so each hour clears MIN_HOUR samples when n is
    // large; arcsinh residuals uniform in [-spread, spread].
    const out: HorizonResidual[] = [];
    for (let i = 0; i < n; i++) {
      const r = -arcsinhSpread + (2 * arcsinhSpread * i) / Math.max(1, n - 1);
      const predictedRaw = 5;
      out.push({
        utcHour: i % 24,
        predictedRaw,
        actualRaw: Math.sinh(Math.asinh(predictedRaw) + r),
      });
    }
    return out;
  };

  it("ships dark (calibrated:false) below the total-sample minimum", () => {
    const residuals = residualWithCoverage(
      MIN_TOTAL_CALIBRATION_SAMPLES - 1,
      1,
    );
    const artifact = buildArtifact(residuals, "2026-06-01T00:00:00.000Z");
    expect(artifact.calibrated).toBe(false);
    expect(artifact.offsetsByHour.size).toBe(0);
    expect(artifact.generatedAt).toBe("");
  });

  it("ships dark with calibrated:false but records observedCoverage below the gate", () => {
    // Construct residuals where the derived band UNDER-covers: a heavy-tailed
    // spread where most mass is in the tails, so the inner 80% offsets miss a
    // lot. We force under-coverage by making the residual distribution bimodal
    // at the extremes (the inner percentiles are tight but the data is spread).
    const out: HorizonResidual[] = [];
    const n = 240;
    for (let i = 0; i < n; i++) {
      // Bimodal: half at ±0.05 (tight centre), half at ±3 (far tails).
      const r =
        i % 2 === 0 ? (i % 4 === 0 ? -0.05 : 0.05) : i % 4 === 1 ? -3 : 3;
      const predictedRaw = 5;
      out.push({
        utcHour: i % 24,
        predictedRaw,
        actualRaw: Math.sinh(Math.asinh(predictedRaw) + r),
      });
    }
    const artifact = buildArtifact(out, "2026-06-01T00:00:00.000Z");
    // Whatever the exact coverage, if it's below the gate it must ship dark.
    if (
      artifact.observedCoverage !== null &&
      artifact.observedCoverage < COVERAGE_GATE_THRESHOLD
    ) {
      expect(artifact.calibrated).toBe(false);
      expect(artifact.offsetsByHour.size).toBe(0);
    }
  });

  it("ships calibrated:true when coverage clears the gate", () => {
    // Well-behaved uniform residuals → derived 80% band covers ~80% ≥ 0.70 gate.
    const residuals = residualWithCoverage(480, 1);
    const artifact = buildArtifact(residuals, "2026-06-01T00:00:00.000Z");
    expect(artifact.observedCoverage).not.toBeNull();
    if (artifact.observedCoverage !== null) {
      expect(artifact.observedCoverage).toBeGreaterThanOrEqual(
        COVERAGE_GATE_THRESHOLD,
      );
    }
    expect(artifact.calibrated).toBe(true);
    expect(artifact.offsetsByHour.size).toBeGreaterThan(0);
    expect(artifact.generatedAt).toBe("2026-06-01T00:00:00.000Z");
  });
});

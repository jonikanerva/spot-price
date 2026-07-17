import { describe, expect, it } from "vitest";
import {
  GO_ATTENUATION,
  MARGINAL_ATTENUATION,
  MIN_BAND_SAMPLES,
  MIN_SAMPLES_PER_BIN,
  recommendation,
  REFERENCE_MAX_LEAD_H,
  runRevisionStudy,
  type RevisionStudyInput,
} from "./revision-magnitude.js";
import { rms, standardDeviation } from "./backtest-metrics.js";
import {
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";
import type { ForecastVintageRecord } from "../src/types.js";

const HOUR = 60 * 60 * 1000;
const iso = (ms: number): string => new Date(ms).toISOString();

/** A vintage issued `leadH` hours before delivery (negative ⇒ post-delivery). */
const vintage = (
  datasetId: number,
  targetMs: number,
  leadH: number,
  value: number,
): ForecastVintageRecord => ({
  datasetId,
  issuedAt: iso(targetMs - leadH * HOUR),
  startTime: iso(targetMs),
  endTime: iso(targetMs + 15 * 60 * 1000),
  value,
});

const wind = (input: RevisionStudyInput) =>
  runRevisionStudy(input).datasets.find(
    (d) => d.datasetId === DATASET_WIND_FORECAST,
  );

const base = Date.parse("2026-06-25T00:00:00.000Z");

describe("reference selection, bucketing, admissibility", () => {
  it("uses the freshest issuance as reference and buckets positive leads", () => {
    const T = Date.parse("2026-07-01T12:00:00.000Z");
    const ds = DATASET_WIND_FORECAST;
    // Reference = post-delivery issuance (freshest, lead −1h); one issuance per
    // distinct lead, each landing in a distinct bin (all thin → stats null).
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          vintage(ds, T, -1, 500),
          vintage(ds, T, 3, 510), // <=6h
          vintage(ds, T, 9, 520), // 6-12h
          vintage(ds, T, 15, 540), // 12-18h
          vintage(ds, T, 21, 570), // 18-24h
          vintage(ds, T, 30, 600), // 24-36h
          vintage(ds, T, 42, 650), // 36-48h
          vintage(ds, T, 60, 700), // >48h
        ],
      },
    };
    const d = wind(input);
    expect(d).toBeDefined();
    if (!d) return;

    expect(d.admissibleTargets).toBe(1);
    expect(d.referencesPostDelivery).toBe(1);
    expect(d.referencesPreDeliveryWithinTol).toBe(0);
    expect(d.futureTargets).toBe(0);
    expect(d.empiricalMaxLeadH).toBeCloseTo(60, 6);

    const byLabel = new Map(d.buckets.map((b) => [b.label, b]));
    for (const label of [
      "<=6h",
      "6-12h",
      "12-18h",
      "18-24h",
      "24-36h",
      "36-48h",
      ">48h",
    ]) {
      const b = byLabel.get(label);
      expect(b?.samples).toBe(1);
      expect(b?.targets).toBe(1);
      // 1 sample < MIN_SAMPLES_PER_BIN → insufficient, stats suppressed.
      expect(b?.sufficient).toBe(false);
      expect(b?.rmsRevision).toBeNull();
    }
  });

  it("prefers a pre-delivery-within-tolerance reference; anchor sets nowProxy", () => {
    const T = Date.parse("2026-07-02T00:00:00.000Z");
    const ds = DATASET_WIND_FORECAST;
    const anchor = T + 3 * HOUR;
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          vintage(ds, T, 1, 1000), // freshest for T, within tol → reference
          vintage(ds, T, 8, 1100), // 6-12h
          vintage(ds, anchor, -1, 900), // post-delivery ref → sets nowProxy ≥ T
        ],
      },
    };
    const d = wind(input);
    expect(d?.referencesPreDeliveryWithinTol).toBe(1);
    expect(d?.referencesPostDelivery).toBe(1);
    expect(d?.admissibleTargets).toBe(2);
    const total = d?.buckets.reduce((s, b) => s + b.samples, 0);
    expect(total).toBe(1); // only T's +8h; both references excluded
    expect(REFERENCE_MAX_LEAD_H).toBe(2);
  });

  it("skips future targets and excludes stale-reference targets", () => {
    const ds = DATASET_WIND_FORECAST;
    const future = base + 200 * HOUR;
    const stale = base + 50 * HOUR;
    const anchor = base + 60 * HOUR; // post-delivery, sets nowProxy past `stale`
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          vintage(ds, base, -1, 300), // admissible anchor near start
          vintage(ds, stale, 6, 700), // freshest lead +6h > 2h → excluded
          vintage(ds, stale, 12, 740),
          vintage(ds, anchor, -1, 800), // admissible, sets nowProxy
          vintage(ds, future, 6, 400), // delivery after nowProxy → future
          vintage(ds, future, 12, 420),
        ],
      },
    };
    const d = wind(input);
    expect(d?.admissibleTargets).toBe(2); // base, anchor
    expect(d?.excludedTargets).toBe(1); // stale
    expect(d?.futureTargets).toBe(1); // future
  });
});

describe("bin statistics (sufficient bins only)", () => {
  it("computes mean/median/p90/rms/bias/NSR/attenuation/relMeanAbs on a known bin", () => {
    const ds = DATASET_WIND_FORECAST;
    const n = 120; // ≥ MIN_SAMPLES_PER_BIN
    const vintages: ForecastVintageRecord[] = [];
    const refs: number[] = [];
    for (let t = 0; t < n; t++) {
      const T = base + t * 6 * HOUR;
      const ref = 1000 + t * 10;
      refs.push(ref);
      vintages.push(vintage(ds, T, -1, ref)); // reference
      // Alternating ±100 revision at lead 12h → 6-12h bin.
      const delta = t % 2 === 0 ? 100 : -100;
      vintages.push(vintage(ds, T, 12, ref + delta));
    }
    const d = wind({ vintagesByDataset: { [String(ds)]: vintages } });
    const bin = d?.buckets.find((b) => b.label === "6-12h");
    expect(bin?.samples).toBe(n);
    expect(bin?.sufficient).toBe(true);
    expect(bin?.meanAbsRevision).toBeCloseTo(100, 6);
    expect(bin?.medianAbsRevision).toBeCloseTo(100, 6);
    expect(bin?.p90AbsRevision).toBeCloseTo(100, 6);
    expect(bin?.rmsRevision).toBeCloseTo(100, 6);
    expect(bin?.signedBiasRevision).toBeCloseTo(0, 6); // ± cancels
    const sdRef = standardDeviation(refs);
    expect(d?.sdReference).toBeCloseTo(sdRef ?? 0, 6);
    const nsr = 100 / (sdRef ?? 1);
    expect(bin?.noiseToSignal).toBeCloseTo(nsr, 6);
    expect(bin?.attenuation).toBeCloseTo(1 / (1 + nsr * nsr), 6);
    const meanRef = refs.reduce((a, b) => a + b, 0) / refs.length;
    expect(bin?.relMeanAbs).toBeCloseTo(100 / meanRef, 6);
  });

  it("marks a bin below MIN_SAMPLES_PER_BIN insufficient with null stats", () => {
    const ds = DATASET_WIND_FORECAST;
    const n = MIN_SAMPLES_PER_BIN - 1; // one short of sufficient
    const vintages: ForecastVintageRecord[] = [];
    for (let t = 0; t < n; t++) {
      const T = base + t * 6 * HOUR;
      vintages.push(vintage(ds, T, -1, 1000));
      vintages.push(vintage(ds, T, 12, 1100));
    }
    const d = wind({ vintagesByDataset: { [String(ds)]: vintages } });
    const bin = d?.buckets.find((b) => b.label === "6-12h");
    expect(bin?.samples).toBe(n);
    expect(bin?.sufficient).toBe(false);
    expect(bin?.meanAbsRevision).toBeNull();
    expect(bin?.rmsRevision).toBeNull();
    // The thin bin is ignored by the aggregate → no band samples.
    expect(d?.productionBandSamples).toBe(0);
  });
});

/**
 * `n` delivered targets, each with a post-delivery reference `refBase+t*refStep`
 * and one issuance per positive lead carrying `reference + revision` (constant),
 * so the reference series has a known sd and every revision equals `revision`.
 */
const genDataset = (
  datasetId: number,
  n: number,
  refBase: number,
  refStep: number,
  revision: number,
  leads: readonly number[] = [6, 12, 24, 36],
): ForecastVintageRecord[] => {
  const out: ForecastVintageRecord[] = [];
  for (let t = 0; t < n; t++) {
    const targetMs = base + t * 6 * HOUR;
    const ref = refBase + t * refStep;
    out.push(vintage(datasetId, targetMs, -1, ref));
    for (const L of leads) {
      out.push(vintage(datasetId, targetMs, L, ref + revision));
    }
  }
  return out;
};

describe("aggregate NSR and attenuation", () => {
  it("aggregates rms(Δ)/sd(reference) over sufficient bins", () => {
    const ds = DATASET_WIND_FORECAST;
    const n = 150; // ≥ MIN_SAMPLES_PER_BIN per bin
    const refStep = 20;
    const revision = 400;
    const d = wind({
      vintagesByDataset: {
        [String(ds)]: genDataset(ds, n, 1000, refStep, revision),
      },
    });
    expect(d).toBeDefined();
    if (!d) return;
    const refValues = Array.from({ length: n }, (_, t) => 1000 + t * refStep);
    const expectedSd = standardDeviation(refValues);
    expect(d.sdReference).toBeCloseTo(expectedSd ?? 0, 6);
    expect(d.productionBandRms).toBeCloseTo(revision, 6);
    expect(rms(Array<number>(n * 4).fill(revision))).toBeCloseTo(revision, 6);
    const nsr = revision / (expectedSd ?? 1);
    expect(d.productionBandNsr).toBeCloseTo(nsr, 6);
    expect(d.attenuationIllustration).toBeCloseTo(1 / (1 + nsr * nsr), 6);
    expect(d.productionBandSamples).toBe(n * 4);
  });

  it("runs the ref-vs-actual sanity check when actuals overlap", () => {
    const ds = DATASET_WIND_FORECAST;
    const T = base;
    const d = wind({
      vintagesByDataset: {
        [String(ds)]: [vintage(ds, T, -1, 500), vintage(ds, T, 12, 560)],
      },
      actualsByDataset: {
        [String(DATASET_WIND_ACTUAL)]: [
          { datasetId: DATASET_WIND_ACTUAL, startTime: iso(T), value: 490 },
        ],
      },
    });
    expect(d?.actualCheck?.targetsCompared).toBe(1);
    expect(d?.actualCheck?.medianAbsRefMinusActual).toBeCloseTo(10, 6);
  });
});

describe("recommendation (GO / MARGINAL / DEFER only)", () => {
  it("GO when attenuation ≤ threshold with enough band samples", () => {
    const ds = DATASET_WIND_FORECAST;
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 150, 1000, 20, 2000) },
    };
    const rec = recommendation(runRevisionStudy(input));
    expect(rec.verdict).toBe("GO");
    const d = wind(input);
    expect(d && (d.attenuationIllustration ?? 1) <= GO_ATTENUATION).toBe(true);
    expect(d && d.productionBandSamples >= MIN_BAND_SAMPLES).toBe(true);
  });

  it("MARGINAL (provisional summer) when attenuation ≥ 0.95", () => {
    const ds = DATASET_CONSUMPTION_FORECAST;
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 150, 8000, 50, 1) },
    };
    const result = runRevisionStudy(input);
    const rec = recommendation(result);
    const d = result.datasets.find(
      (x) => x.datasetId === DATASET_CONSUMPTION_FORECAST,
    );
    expect((d?.attenuationIllustration ?? 0) >= MARGINAL_ATTENUATION).toBe(
      true,
    );
    expect(rec.verdict).toBe("MARGINAL");
    expect(rec.reason).toContain("PROVISIONAL");
    expect(rec.reason).toContain("#80");
  });

  it("MARGINAL (modest) when attenuation is in (0.90, 0.95)", () => {
    const ds = DATASET_WIND_FORECAST;
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 150, 1000, 20, 250) },
    };
    const result = runRevisionStudy(input);
    const rec = recommendation(result);
    const att = wind(input)?.attenuationIllustration ?? 0;
    expect(att).toBeGreaterThan(GO_ATTENUATION);
    expect(att).toBeLessThan(MARGINAL_ATTENUATION);
    expect(rec.verdict).toBe("MARGINAL");
    expect(rec.reason).toContain("#80");
  });

  it("DEFERs when every bin is thin (no sufficient band samples)", () => {
    const ds = DATASET_WIND_FORECAST;
    // 50 targets → 50 samples/bin < MIN_SAMPLES_PER_BIN → all thin.
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 50, 1000, 20, 2000) },
    };
    const rec = recommendation(runRevisionStudy(input));
    expect(rec.verdict).toBe("DEFER");
  });

  it("never recommends closing #81 (da amendment 1)", () => {
    for (const revision of [1, 250, 2000]) {
      const ds = DATASET_WIND_FORECAST;
      const input: RevisionStudyInput = {
        vintagesByDataset: {
          [String(ds)]: genDataset(ds, 150, 1000, 20, revision),
        },
      };
      const rec = recommendation(runRevisionStudy(input));
      expect(["GO", "MARGINAL", "DEFER"]).toContain(rec.verdict);
      expect(rec.reason.toLowerCase()).not.toContain("close #81 as");
    }
  });
});

describe("empty input", () => {
  it("returns a null window and empty per-dataset summaries", () => {
    const result = runRevisionStudy({ vintagesByDataset: {} });
    expect(result.window).toBeNull();
    expect(result.datasets).toHaveLength(2);
    for (const d of result.datasets) {
      expect(d.admissibleTargets).toBe(0);
      expect(d.productionBandSamples).toBe(0);
    }
    expect(recommendation(result).verdict).toBe("DEFER");
  });
});

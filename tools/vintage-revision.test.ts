import { describe, expect, it } from "vitest";
import {
  GO_ATTENUATION,
  MIN_BAND_SAMPLES,
  REFERENCE_MAX_LEAD_H,
  recommendation,
  runRevisionStudy,
  type RevisionStudyInput,
  type VintageRecord,
} from "./vintage-revision.js";
import { median, percentile, rms, sd } from "./backtest-metrics.js";
import {
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";

const HOUR = 60 * 60 * 1000;
const iso = (ms: number): string => new Date(ms).toISOString();

/** A vintage issued `leadH` hours before delivery (negative ⇒ post-delivery). */
const vintage = (
  datasetId: number,
  targetMs: number,
  leadH: number,
  value: number,
): VintageRecord => ({
  datasetId,
  issuedAt: iso(targetMs - leadH * HOUR),
  startTime: iso(targetMs),
  value,
});

const wind = (input: RevisionStudyInput) =>
  runRevisionStudy(input).datasets.find(
    (d) => d.datasetId === DATASET_WIND_FORECAST,
  );

describe("metric helpers", () => {
  it("median handles odd, even, and empty", () => {
    expect(median([3, 1, 2])).toBe(2);
    expect(median([4, 1, 2, 3])).toBe(2.5);
    expect(median([])).toBeNull();
  });

  it("percentile interpolates and clamps", () => {
    expect(percentile([0, 10], 50)).toBe(5);
    expect(percentile([0, 10, 20, 30], 90)).toBeCloseTo(27, 6);
    expect(percentile([5], 90)).toBe(5);
    expect(percentile([], 90)).toBeNull();
  });

  it("sd is the sample (n−1) standard deviation, null below 2", () => {
    // values 2,4,6 → mean 4, var = (4+0+4)/2 = 4 → sd 2
    expect(sd([2, 4, 6])).toBeCloseTo(2, 6);
    expect(sd([7])).toBeNull();
    expect(sd([])).toBeNull();
  });

  it("rms is the quadratic mean, null on empty", () => {
    expect(rms([3, 4])).toBeCloseTo(Math.sqrt(12.5), 6);
    expect(rms([])).toBeNull();
  });
});

describe("reference selection and bucketing", () => {
  it("uses the freshest issuance as reference and buckets positive leads", () => {
    const T = Date.parse("2026-07-01T12:00:00.000Z");
    const ds = DATASET_WIND_FORECAST;
    // Reference = post-delivery issuance (freshest, lead −1h), value 500.
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          vintage(ds, T, -1, 500), // freshest → reference
          vintage(ds, T, 3, 510), // <=6h   → +10
          vintage(ds, T, 9, 520), // 6-12h  → +20
          vintage(ds, T, 15, 540), // 12-18h → +40
          vintage(ds, T, 21, 570), // 18-24h → +70
          vintage(ds, T, 30, 600), // 24-36h → +100
          vintage(ds, T, 42, 650), // 36-48h → +150
          vintage(ds, T, 60, 700), // >48h   → +200
        ],
      },
    };
    const d = wind(input);
    expect(d).toBeDefined();
    if (!d) return;

    expect(d.admissibleTargets).toBe(1);
    expect(d.excludedTargets).toBe(0);
    expect(d.futureTargets).toBe(0);
    expect(d.referencesPostDelivery).toBe(1);
    expect(d.referencesPreDeliveryWithinTol).toBe(0);
    expect(d.empiricalMaxLeadH).toBeCloseTo(60, 6);

    const signedByLabel = new Map(
      d.buckets.map((b) => [b.label, b.medianSignedRevision]),
    );
    expect(signedByLabel.get("<=6h")).toBeCloseTo(10, 6);
    expect(signedByLabel.get("6-12h")).toBeCloseTo(20, 6);
    expect(signedByLabel.get("12-18h")).toBeCloseTo(40, 6);
    expect(signedByLabel.get("18-24h")).toBeCloseTo(70, 6);
    expect(signedByLabel.get("24-36h")).toBeCloseTo(100, 6);
    expect(signedByLabel.get("36-48h")).toBeCloseTo(150, 6);
    expect(signedByLabel.get(">48h")).toBeCloseTo(200, 6);
    for (const b of d.buckets) {
      expect(b.samples).toBe(1);
      expect(b.targets).toBe(1);
    }
  });

  it("prefers a pre-delivery-within-tolerance reference and excludes it from Δ", () => {
    const T = Date.parse("2026-07-02T00:00:00.000Z");
    const ds = DATASET_WIND_FORECAST;
    // Target T's freshest issuance is +1h before delivery (within tolerance).
    // A separate anchor delivered AFTER T carries a post-delivery issuance, so
    // nowProxy ≥ T and T is NOT treated as future (a target whose freshest
    // issuance is still pre-delivery with no later anchor is correctly "future").
    const anchor = T + 3 * HOUR;
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          vintage(ds, T, 1, 1000), // freshest for T, within tol → reference (excluded)
          vintage(ds, T, 8, 1100), // 6-12h → +100
          vintage(ds, anchor, -1, 900), // post-delivery ref for the anchor → sets nowProxy
        ],
      },
    };
    const d = wind(input);
    expect(d?.referencesPreDeliveryWithinTol).toBe(1); // target T
    expect(d?.referencesPostDelivery).toBe(1); // anchor
    expect(d?.admissibleTargets).toBe(2);
    // Only T's +8h observation contributes; both reference rows are excluded.
    const total = d?.buckets.reduce((s, b) => s + b.samples, 0);
    expect(total).toBe(1);
    expect(REFERENCE_MAX_LEAD_H).toBe(2);
  });
});

describe("admissibility", () => {
  it("skips future targets (delivery after the last observed issuance)", () => {
    const ds = DATASET_WIND_FORECAST;
    const anchor = Date.parse("2026-07-01T00:00:00.000Z");
    const future = anchor + 20 * HOUR; // delivered after nowProxy
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          // anchor: post-delivery ref sets nowProxy = anchor+1h
          vintage(ds, anchor, -1, 300),
          vintage(ds, anchor, 6, 320),
          // future target: only pre-delivery issuances, all before its delivery
          vintage(ds, future, 6, 400),
          vintage(ds, future, 12, 420),
        ],
      },
    };
    const d = wind(input);
    expect(d?.admissibleTargets).toBe(1);
    expect(d?.futureTargets).toBe(1);
    expect(d?.excludedTargets).toBe(0);
  });

  it("excludes a delivered target whose freshest lead exceeds the cutoff", () => {
    const ds = DATASET_WIND_FORECAST;
    const c = Date.parse("2026-07-01T00:00:00.000Z");
    const anchor = c + 3 * HOUR;
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [
          // C: freshest issuance is +6h before delivery → stale reference
          vintage(ds, c, 6, 700),
          vintage(ds, c, 12, 740),
          // anchor after C sets nowProxy past C so C is not "future"
          vintage(ds, anchor, -1, 800),
        ],
      },
    };
    const d = wind(input);
    expect(d?.excludedTargets).toBe(1); // C
    expect(d?.admissibleTargets).toBe(1); // anchor
    expect(d?.futureTargets).toBe(0);
    expect(d?.exclusionRate).toBeCloseTo(0.5, 6);
  });
});

// ---------------------------------------------------------------------------
// A synthetic dataset with a controllable revision magnitude, for the
// noise-to-signal / attenuation / recommendation assertions.
// ---------------------------------------------------------------------------

/**
 * `n` delivered targets, each with a post-delivery reference `refBase+t*refStep`
 * and one issuance per positive lead carrying `reference + revision`. So every
 * revision equals `revision` (constant) and the reference series has a known sd.
 */
const genDataset = (
  datasetId: number,
  n: number,
  refBase: number,
  refStep: number,
  revision: number,
  leads: readonly number[] = [6, 12, 24, 36],
): VintageRecord[] => {
  const base = Date.parse("2026-06-25T00:00:00.000Z");
  const out: VintageRecord[] = [];
  for (let t = 0; t < n; t++) {
    const targetMs = base + t * 6 * HOUR;
    const ref = refBase + t * refStep;
    out.push(vintage(datasetId, targetMs, -1, ref)); // freshest → reference
    for (const L of leads) {
      out.push(vintage(datasetId, targetMs, L, ref + revision));
    }
  }
  return out;
};

describe("noise-to-signal and attenuation", () => {
  it("computes NSR = rms(Δ)/sd(reference) and attenuation = 1/(1+NSR²)", () => {
    const ds = DATASET_WIND_FORECAST;
    const n = 60;
    const refStep = 20;
    const revision = 400;
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: genDataset(ds, n, 1000, refStep, revision),
      },
    };
    const d = wind(input);
    expect(d).toBeDefined();
    if (!d) return;

    const refValues = Array.from({ length: n }, (_, t) => 1000 + t * refStep);
    const expectedSd = sd(refValues);
    expect(d.sdReference).toBeCloseTo(expectedSd ?? 0, 6);
    // All revisions equal `revision`, so the band rms is exactly |revision|.
    expect(d.productionBandRms).toBeCloseTo(revision, 6);
    const expectedNsr = revision / (expectedSd ?? 1);
    expect(d.productionBandNsr).toBeCloseTo(expectedNsr, 6);
    expect(d.attenuationIllustration).toBeCloseTo(
      1 / (1 + expectedNsr * expectedNsr),
      6,
    );
    expect(d.productionBandSamples).toBe(n * 4);
  });

  it("runs the ref-vs-actual sanity check when actuals overlap", () => {
    const ds = DATASET_WIND_FORECAST;
    const base = Date.parse("2026-06-25T00:00:00.000Z");
    const T = base;
    const input: RevisionStudyInput = {
      vintagesByDataset: {
        [String(ds)]: [vintage(ds, T, -1, 500), vintage(ds, T, 12, 560)],
      },
      actualsByDataset: {
        [String(DATASET_WIND_ACTUAL)]: [
          { datasetId: DATASET_WIND_ACTUAL, startTime: iso(T), value: 490 },
        ],
      },
    };
    const d = wind(input);
    expect(d?.actualCheck?.targetsCompared).toBe(1);
    // reference 500 vs actual 490 → |diff| 10
    expect(d?.actualCheck?.medianAbsRefMinusActual).toBeCloseTo(10, 6);
  });
});

describe("recommendation (GO / MARGINAL / DEFER only)", () => {
  it("GO when attenuation ≤ threshold on a dataset with enough samples", () => {
    const ds = DATASET_WIND_FORECAST;
    // Large revision vs modest reference spread → NSR ≫ 1 → attenuation ≪ 0.90.
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 60, 1000, 20, 2000) },
    };
    const rec = recommendation(runRevisionStudy(input));
    expect(rec.verdict).toBe("GO");
    const d = wind(input);
    expect(d && (d.attenuationIllustration ?? 1) <= GO_ATTENUATION).toBe(true);
    expect(d && d.productionBandSamples >= MIN_BAND_SAMPLES).toBe(true);
  });

  it("MARGINAL when the revision is tiny relative to the signal", () => {
    const ds = DATASET_CONSUMPTION_FORECAST;
    // Tiny revision vs wide reference spread → NSR ≈ 0 → attenuation ≈ 1.
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 60, 8000, 50, 1) },
    };
    const rec = recommendation(runRevisionStudy(input));
    expect(rec.verdict).toBe("MARGINAL");
    // MARGINAL points at #80, and explicitly does NOT close #81 outright.
    expect(rec.reason).toContain("#80");
  });

  it("DEFERs when no dataset reaches the minimum band samples", () => {
    const ds = DATASET_WIND_FORECAST;
    const input: RevisionStudyInput = {
      vintagesByDataset: { [String(ds)]: genDataset(ds, 10, 1000, 20, 2000) },
    };
    const rec = recommendation(runRevisionStudy(input));
    expect(rec.verdict).toBe("DEFER");
  });

  it("never recommends closing #81 (da amendment 1)", () => {
    for (const revision of [1, 50, 2000]) {
      const ds = DATASET_WIND_FORECAST;
      const input: RevisionStudyInput = {
        vintagesByDataset: {
          [String(ds)]: genDataset(ds, 60, 1000, 20, revision),
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

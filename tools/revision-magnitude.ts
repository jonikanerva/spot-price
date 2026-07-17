/**
 * OFFLINE / DEV-ONLY analysis ENGINE for issue #79 — quantify how much the
 * Fingrid FORECAST datasets (wind 245, consumption 165) get revised between
 * early issuance and delivery, and translate that into a GO / MARGINAL / DEFER
 * recommendation for the vintage-correct model training in #81.
 *
 * This is a PURE LIBRARY: no `main`, no I/O (no `process.env`, no network, no
 * DB, no clock). It lives in `tools/` so it can never reach the production
 * bundle (tsup's only entry is `src/index.ts`; the ESLint guard forbids `src/`
 * runtime from importing `tools/`). The single runnable entry point is
 * `tools/revision-magnitude-cli.ts`, which loads the vintages off the DB (via
 * `getFingridForecastVintagesAll`) and feeds them here. It adds NO background
 * job (`STACK §9`) and NO endpoint.
 *
 * The idea (errors-in-variables): the leaky training/backtest fed the model the
 * FINAL (≈near-delivery) forecast value for every past quarter — that is what
 * the pre-#78 upsert-latest storage kept. At serve time the model instead sees
 * a rough +12…+44h forecast. If those two differ a lot, the fit is calibrated
 * on cleaner data than it is applied to and over-trusts wind/consumption at long
 * horizons; if they barely differ, the whole #78→#81 chain is not worth it. This
 * engine measures that difference (the "revision") as a function of lead time.
 *
 * Reference = the FRESHEST vintage per target (max `issued_at`), i.e. exactly
 * the value upsert-latest would have kept and fed the leaky pipeline. A revision
 * at lead L is `value@L − reference`. Metrics are reported per dataset and per
 * lead-time bin. All arithmetic is on UTC epoch ms parsed from ISO strings
 * (`VISION.md → UTC internally`).
 */
import {
  median,
  percentile,
  rms,
  standardDeviation,
} from "./backtest-metrics.js";
import {
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";
import type { ForecastVintageRecord } from "../src/types.js";

const HOUR_MS = 60 * 60 * 1000;

/** The forecast datasets whose vintages this study quantifies, in report order. */
export const VINTAGE_DATASET_IDS: readonly number[] = [
  DATASET_WIND_FORECAST,
  DATASET_CONSUMPTION_FORECAST,
];

/**
 * A target is admissible only if its freshest issuance is within this many hours
 * of delivery, so the reference is a genuine near-delivery value and not a stale
 * mid-horizon forecast. Inherits the ±1h `issued_at` fetch-time-proxy
 * uncertainty (migration 005), so 2h ≈ "the last issuance or two before/after
 * delivery". A negative lead (issuance postdates delivery) is also admissible —
 * that is the settled value, the best possible reference.
 */
export const REFERENCE_MAX_LEAD_H = 2;

/**
 * Lead-time bins `(loH, hiH]` in hours. Edges are finer near delivery (where the
 * served forecast leans hardest on the freshest forecast) and coarser far out,
 * and straddle the two datasets' empirical ladders (wind 245 reaches ~+72h,
 * consumption 165 ~+24h). Bins beyond a dataset's ladder are simply empty.
 */
export const LEAD_BUCKETS: readonly {
  readonly label: string;
  readonly loH: number;
  readonly hiH: number;
}[] = [
  { label: "<=6h", loH: 0, hiH: 6 },
  { label: "6-12h", loH: 6, hiH: 12 },
  { label: "12-18h", loH: 12, hiH: 18 },
  { label: "18-24h", loH: 18, hiH: 24 },
  { label: "24-36h", loH: 24, hiH: 36 },
  { label: "36-48h", loH: 36, hiH: 48 },
  { label: ">48h", loH: 48, hiH: Number.POSITIVE_INFINITY },
];

/**
 * Below this many revision observations a bin is "insufficient": its stats are
 * suppressed and it is IGNORED by the recommendation and the dataset aggregate
 * (a near-empty bin — e.g. consumption's long-lead bins — is not evidence). The
 * sample COUNT is always reported so the emptiness is visible.
 */
export const MIN_SAMPLES_PER_BIN = 100;

/**
 * Minimum revision observations (summed over a dataset's sufficient bins) before
 * its attenuation may drive the verdict. Below this the recommendation DEFERs.
 */
export const MIN_BAND_SAMPLES = 200;

/** GO threshold: attenuation at or below this on either dataset earns #81. */
export const GO_ATTENUATION = 0.9;

/** At/above this the effect "barely moves" → MARGINAL, provisional (summer). */
export const MARGINAL_ATTENUATION = 0.95;

// ---------------------------------------------------------------------------
// Inputs (public grid data only — no user data)
// ---------------------------------------------------------------------------

/**
 * One actual (settled) value for a target quarter, used ONLY as a secondary
 * sanity check that the freshest-forecast reference really is near-actual
 * (da amendment 4). Optional — the study runs without it.
 */
export interface ActualRecord {
  readonly datasetId: number;
  readonly startTime: string;
  readonly value: number;
}

export interface RevisionStudyInput {
  /** Forecast vintages keyed by string dataset id (as the store produces them). */
  readonly vintagesByDataset: Readonly<
    Record<string, readonly ForecastVintageRecord[]>
  >;
  /** Optional settled actuals (75 wind, 124 consumption) for the sanity check. */
  readonly actualsByDataset?: Readonly<Record<string, readonly ActualRecord[]>>;
}

// ---------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------

/**
 * Per (dataset, lead bin) revision statistics. When `sufficient` is false
 * (samples < MIN_SAMPLES_PER_BIN) the stat fields are null — the count is still
 * reported, but the bin is not treated as evidence.
 */
export interface BucketMetrics {
  readonly label: string;
  readonly loH: number;
  readonly hiH: number;
  /** Number of revision observations (target × issuance pairs). */
  readonly samples: number;
  /** Number of distinct targets contributing. */
  readonly targets: number;
  /** Whether the bin has enough samples to be treated as evidence. */
  readonly sufficient: boolean;
  /** Mean absolute revision (MW). */
  readonly meanAbsRevision: number | null;
  /** Median absolute revision (MW). */
  readonly medianAbsRevision: number | null;
  /** 90th-percentile absolute revision (MW). */
  readonly p90AbsRevision: number | null;
  /** RMS revision (MW) — the errors-in-variables "noise" amplitude at this lead. */
  readonly rmsRevision: number | null;
  /** Mean SIGNED revision (MW) — a systematic early-forecast bias if non-zero. */
  readonly signedBiasRevision: number | null;
  /** rmsRevision / sd(reference series) — noise-to-signal at this lead. */
  readonly noiseToSignal: number | null;
  /** 1/(1+NSR²) — labelled single-variable errors-in-variables illustration. */
  readonly attenuation: number | null;
  /** meanAbsRevision / mean(|reference|) — revision as a share of the level. */
  readonly relMeanAbs: number | null;
}

/** Reference-vs-actual sanity result (da amendment 4). */
export interface ActualCheck {
  readonly targetsCompared: number;
  readonly medianAbsRefMinusActual: number | null;
  readonly meanAbsRefMinusActual: number | null;
}

export interface DatasetRevisionSummary {
  readonly datasetId: number;
  /** Targets whose freshest issuance was within REFERENCE_MAX_LEAD_H of delivery. */
  readonly admissibleTargets: number;
  /** Targets excluded because the freshest issuance never got near delivery. */
  readonly excludedTargets: number;
  /** Targets skipped because delivery is after the last observed issuance (future). */
  readonly futureTargets: number;
  /** Fraction of considered (admissible + excluded) targets that were excluded. */
  readonly exclusionRate: number | null;
  /** Admissible references whose issuance postdates delivery (settled value). */
  readonly referencesPostDelivery: number;
  /** Admissible references issued within tolerance BEFORE delivery. */
  readonly referencesPreDeliveryWithinTol: number;
  /** Largest positive lead observed (h) — the dataset's empirical forecast reach. */
  readonly empiricalMaxLeadH: number | null;
  /** 90th-percentile positive lead (h). */
  readonly empiricalP90LeadH: number | null;
  /** sd of the near-delivery reference series (MW) — the "signal" amplitude. */
  readonly sdReference: number | null;
  /** mean(|reference|) (MW) — the typical level for relative reporting. */
  readonly meanAbsReference: number | null;
  /** Aggregate RMS revision over the SUFFICIENT bins only (MW). */
  readonly productionBandRms: number | null;
  /** productionBandRms / sdReference — the aggregate noise-to-signal ratio. */
  readonly productionBandNsr: number | null;
  /**
   * Aggregate `1 / (1 + NSR²)` over the sufficient bins — the classic OLS
   * attenuation factor for one noisy regressor. Deliberately a derived,
   * explicitly-labelled illustration, NOT the multivariate factor the real ridge
   * fit applies (da amendment 3). Lower ⇒ the leaky fit over-trusts the feature
   * more ⇒ more to gain from #81.
   */
  readonly attenuationIllustration: number | null;
  /** Total revision observations in the sufficient bins. */
  readonly productionBandSamples: number;
  readonly actualCheck: ActualCheck | null;
  readonly buckets: readonly BucketMetrics[];
}

export interface RevisionWindow {
  readonly earliestIssuedAt: string;
  readonly latestIssuedAt: string;
  readonly earliestTarget: string;
  readonly latestTarget: string;
}

export interface RevisionStudyResult {
  readonly datasets: readonly DatasetRevisionSummary[];
  /** UTC span of the data, or null when there are no vintages at all. */
  readonly window: RevisionWindow | null;
}

const msOf = (iso: string): number => new Date(iso).getTime();

/** Mean of the values, or null when empty. */
const meanOrNull = (xs: readonly number[]): number | null =>
  xs.length === 0 ? null : xs.reduce((a, b) => a + b, 0) / xs.length;

/**
 * Max of the values, or null when empty. A reduce — NOT `Math.max(...xs)`, which
 * blows the call stack when spread over the hundreds of thousands of leads a
 * production window produces.
 */
const maxOrNull = (xs: readonly number[]): number | null => {
  if (xs.length === 0) {
    return null;
  }
  let max = -Infinity;
  for (const x of xs) {
    if (x > max) {
      max = x;
    }
  }
  return max;
};

/** The bin a positive lead (hours) falls into, or null if lead ≤ 0. */
const bucketIndexOf = (leadH: number): number | null => {
  if (leadH <= 0) {
    return null;
  }
  for (let i = 0; i < LEAD_BUCKETS.length; i++) {
    const b = LEAD_BUCKETS[i];
    if (b !== undefined && leadH > b.loH && leadH <= b.hiH) {
      return i;
    }
  }
  return null;
};

interface TargetOutcome {
  readonly admissible: boolean;
  readonly future: boolean;
  readonly refValue: number;
  readonly refLeadMs: number;
  /** Positive-lead (leadMs, revision) observations, reference row excluded. */
  readonly observations: readonly { leadMs: number; revision: number }[];
}

/**
 * Classify one target's vintages: pick the freshest as the reference, decide
 * admissibility against `nowProxyMs` and REFERENCE_MAX_LEAD_H, and collect the
 * positive-lead revisions relative to the reference. Pure.
 */
const classifyTarget = (
  targetMs: number,
  vintages: readonly ForecastVintageRecord[],
  nowProxyMs: number,
): TargetOutcome => {
  const empty: TargetOutcome = {
    admissible: false,
    future: false,
    refValue: 0,
    refLeadMs: 0,
    observations: [],
  };
  if (vintages.length === 0) {
    return empty;
  }

  // Freshest = max issued_at. This is exactly what upsert-latest kept and fed
  // the leaky training/backtest, so it is the correct reference to measure the
  // train/serve skew against.
  let ref = vintages[0];
  if (ref === undefined) {
    return empty;
  }
  let refIssuedMs = msOf(ref.issuedAt);
  for (const v of vintages) {
    const issuedMs = msOf(v.issuedAt);
    if (issuedMs > refIssuedMs) {
      ref = v;
      refIssuedMs = issuedMs;
    }
  }

  // Future: delivery is after the last time the job ran, so no near-delivery
  // reference (and no realised value) exists yet — cannot measure revision.
  if (targetMs > nowProxyMs) {
    return { ...empty, future: true };
  }

  const refLeadMs = targetMs - refIssuedMs;
  if (refLeadMs > REFERENCE_MAX_LEAD_H * HOUR_MS) {
    // Stale reference: the freshest issuance never got near delivery (e.g. a
    // job gap), so it is not a trustworthy near-delivery value → exclude.
    return { ...empty, refLeadMs };
  }

  const observations: { leadMs: number; revision: number }[] = [];
  for (const v of vintages) {
    if (v === ref) {
      continue;
    }
    const leadMs = targetMs - msOf(v.issuedAt);
    if (leadMs <= 0) {
      // Post-delivery duplicate of the settled value — not a forecast horizon.
      continue;
    }
    observations.push({ leadMs, revision: v.value - ref.value });
  }

  return {
    admissible: true,
    future: false,
    refValue: ref.value,
    refLeadMs,
    observations,
  };
};

interface BucketAccumulator {
  readonly revisions: number[];
  readonly targets: Set<number>;
}

const summariseDataset = (
  datasetId: number,
  vintages: readonly ForecastVintageRecord[],
  actuals: readonly ActualRecord[],
): DatasetRevisionSummary => {
  if (vintages.length === 0) {
    return {
      datasetId,
      admissibleTargets: 0,
      excludedTargets: 0,
      futureTargets: 0,
      exclusionRate: null,
      referencesPostDelivery: 0,
      referencesPreDeliveryWithinTol: 0,
      empiricalMaxLeadH: null,
      empiricalP90LeadH: null,
      sdReference: null,
      meanAbsReference: null,
      productionBandRms: null,
      productionBandNsr: null,
      attenuationIllustration: null,
      productionBandSamples: 0,
      actualCheck: null,
      buckets: LEAD_BUCKETS.map((b) => emptyBucket(b)),
    };
  }

  // Group vintages by target quarter.
  const byTarget = new Map<number, ForecastVintageRecord[]>();
  let nowProxyMs = -Infinity;
  for (const v of vintages) {
    const issuedMs = msOf(v.issuedAt);
    if (issuedMs > nowProxyMs) {
      nowProxyMs = issuedMs;
    }
    const targetMs = msOf(v.startTime);
    const bucket = byTarget.get(targetMs) ?? [];
    bucket.push(v);
    byTarget.set(targetMs, bucket);
  }

  const bucketAcc: BucketAccumulator[] = LEAD_BUCKETS.map(() => ({
    revisions: [],
    targets: new Set<number>(),
  }));
  const refValues: number[] = [];
  const allPositiveLeadsH: number[] = [];
  const refByTarget = new Map<number, number>();

  let admissibleTargets = 0;
  let excludedTargets = 0;
  let futureTargets = 0;
  let referencesPostDelivery = 0;
  let referencesPreDeliveryWithinTol = 0;

  for (const [targetMs, group] of byTarget) {
    const outcome = classifyTarget(targetMs, group, nowProxyMs);
    if (outcome.future) {
      futureTargets++;
      continue;
    }
    if (!outcome.admissible) {
      excludedTargets++;
      continue;
    }
    admissibleTargets++;
    refValues.push(outcome.refValue);
    refByTarget.set(targetMs, outcome.refValue);
    if (outcome.refLeadMs <= 0) {
      referencesPostDelivery++;
    } else {
      referencesPreDeliveryWithinTol++;
    }
    for (const obs of outcome.observations) {
      const leadH = obs.leadMs / HOUR_MS;
      allPositiveLeadsH.push(leadH);
      const idx = bucketIndexOf(leadH);
      if (idx !== null) {
        const acc = bucketAcc[idx];
        if (acc !== undefined) {
          acc.revisions.push(obs.revision);
          acc.targets.add(targetMs);
        }
      }
    }
  }

  const sdReference = standardDeviation(refValues);
  const meanAbsReference = meanOrNull(refValues.map((v) => Math.abs(v)));

  const buckets: BucketMetrics[] = LEAD_BUCKETS.map((b, i) => {
    const acc = bucketAcc[i] ?? { revisions: [], targets: new Set<number>() };
    return bucketMetrics(b, acc, sdReference, meanAbsReference);
  });

  // Aggregate over SUFFICIENT bins only (thin bins ignored — da / final design).
  const bandRevisions: number[] = [];
  for (let i = 0; i < LEAD_BUCKETS.length; i++) {
    const acc = bucketAcc[i];
    if (acc !== undefined && acc.revisions.length >= MIN_SAMPLES_PER_BIN) {
      bandRevisions.push(...acc.revisions);
    }
  }
  const productionBandRms = rms(bandRevisions);
  const productionBandNsr = ratioOrNull(productionBandRms, sdReference);
  const attenuationIllustration =
    productionBandNsr === null ? null : attenuationOf(productionBandNsr);

  const consideredTargets = admissibleTargets + excludedTargets;

  return {
    datasetId,
    admissibleTargets,
    excludedTargets,
    futureTargets,
    exclusionRate:
      consideredTargets > 0 ? excludedTargets / consideredTargets : null,
    referencesPostDelivery,
    referencesPreDeliveryWithinTol,
    empiricalMaxLeadH: maxOrNull(allPositiveLeadsH),
    empiricalP90LeadH: percentile(allPositiveLeadsH, 90),
    sdReference,
    meanAbsReference,
    productionBandRms,
    productionBandNsr,
    attenuationIllustration,
    productionBandSamples: bandRevisions.length,
    actualCheck: actualCheckOf(refByTarget, actuals),
    buckets,
  };
};

const emptyBucket = (b: {
  label: string;
  loH: number;
  hiH: number;
}): BucketMetrics => ({
  label: b.label,
  loH: b.loH,
  hiH: b.hiH,
  samples: 0,
  targets: 0,
  sufficient: false,
  meanAbsRevision: null,
  medianAbsRevision: null,
  p90AbsRevision: null,
  rmsRevision: null,
  signedBiasRevision: null,
  noiseToSignal: null,
  attenuation: null,
  relMeanAbs: null,
});

const bucketMetrics = (
  b: { label: string; loH: number; hiH: number },
  acc: BucketAccumulator,
  sdReference: number | null,
  meanAbsReference: number | null,
): BucketMetrics => {
  const samples = acc.revisions.length;
  const sufficient = samples >= MIN_SAMPLES_PER_BIN;
  if (!sufficient) {
    return {
      ...emptyBucket(b),
      samples,
      targets: acc.targets.size,
    };
  }
  const abs = acc.revisions.map((r) => Math.abs(r));
  const rmsRevision = rms(acc.revisions);
  const meanAbsRevision = meanOrNull(abs);
  const noiseToSignal = ratioOrNull(rmsRevision, sdReference);
  return {
    label: b.label,
    loH: b.loH,
    hiH: b.hiH,
    samples,
    targets: acc.targets.size,
    sufficient: true,
    meanAbsRevision,
    medianAbsRevision: median(abs),
    p90AbsRevision: percentile(abs, 90),
    rmsRevision,
    signedBiasRevision: meanOrNull(acc.revisions),
    noiseToSignal,
    attenuation: noiseToSignal === null ? null : attenuationOf(noiseToSignal),
    relMeanAbs: ratioOrNull(meanAbsRevision, meanAbsReference),
  };
};

/** The single-variable errors-in-variables attenuation factor for a given NSR. */
const attenuationOf = (nsr: number): number => 1 / (1 + nsr * nsr);

/** `a / b`, or null when either is null or the denominator is ~0. */
const ratioOrNull = (a: number | null, b: number | null): number | null => {
  if (a === null || b === null || Math.abs(b) < 1e-12) {
    return null;
  }
  return a / b;
};

/**
 * Compare the freshest-forecast reference against the settled actual for the
 * same target (secondary sanity check that the reference ≈ actual). Null when
 * no actuals overlap the admissible targets.
 */
const actualCheckOf = (
  refByTarget: ReadonlyMap<number, number>,
  actuals: readonly ActualRecord[],
): ActualCheck | null => {
  if (actuals.length === 0 || refByTarget.size === 0) {
    return null;
  }
  const actualByTarget = new Map<number, number>();
  for (const a of actuals) {
    actualByTarget.set(msOf(a.startTime), a.value);
  }
  const diffs: number[] = [];
  for (const [targetMs, refValue] of refByTarget) {
    const actual = actualByTarget.get(targetMs);
    if (actual !== undefined) {
      diffs.push(Math.abs(refValue - actual));
    }
  }
  if (diffs.length === 0) {
    return null;
  }
  return {
    targetsCompared: diffs.length,
    medianAbsRefMinusActual: median(diffs),
    meanAbsRefMinusActual: meanOrNull(diffs),
  };
};

/** Run the full study over the provided vintages. Pure. */
export const runRevisionStudy = (
  input: RevisionStudyInput,
): RevisionStudyResult => {
  const datasets = VINTAGE_DATASET_IDS.map((id) =>
    summariseDataset(
      id,
      input.vintagesByDataset[String(id)] ?? [],
      input.actualsByDataset?.[String(actualIdFor(id))] ?? [],
    ),
  );

  let earliestIssued = Infinity;
  let latestIssued = -Infinity;
  let earliestTarget = Infinity;
  let latestTarget = -Infinity;
  let any = false;
  for (const records of Object.values(input.vintagesByDataset)) {
    for (const v of records) {
      any = true;
      const issued = msOf(v.issuedAt);
      const target = msOf(v.startTime);
      if (issued < earliestIssued) earliestIssued = issued;
      if (issued > latestIssued) latestIssued = issued;
      if (target < earliestTarget) earliestTarget = target;
      if (target > latestTarget) latestTarget = target;
    }
  }

  const window: RevisionWindow | null = any
    ? {
        earliestIssuedAt: new Date(earliestIssued).toISOString(),
        latestIssuedAt: new Date(latestIssued).toISOString(),
        earliestTarget: new Date(earliestTarget).toISOString(),
        latestTarget: new Date(latestTarget).toISOString(),
      }
    : null;

  return { datasets, window };
};

/** Map a forecast dataset id to its paired actual dataset id (for the check). */
const actualIdFor = (forecastId: number): number => {
  if (forecastId === DATASET_WIND_FORECAST) {
    return 75; // DATASET_WIND_ACTUAL
  }
  if (forecastId === DATASET_CONSUMPTION_FORECAST) {
    return 124; // DATASET_CONSUMPTION_ACTUAL
  }
  return forecastId;
};

// ---------------------------------------------------------------------------
// Recommendation (da amendments 1, 2, 6 + architect final design)
// ---------------------------------------------------------------------------

export type Recommendation = "GO" | "MARGINAL" | "DEFER";

export interface RecommendationResult {
  readonly verdict: Recommendation;
  readonly reason: string;
}

const fmt = (v: number | null, dec = 3): string =>
  v !== null ? v.toFixed(dec) : "n/a";

/**
 * GO / MARGINAL / DEFER only — NEVER a terminal "close #81" (da amendment 1). A
 * close can only be recorded later, after #80's measured backtest delta or a
 * winter re-measure, since this data is summer-only.
 *
 *   - DEFER  — no dataset has enough sufficient-bin samples to judge; re-measure
 *              once more vintages (ideally a winter regime) have accrued.
 *   - GO     — attenuation ≤ GO_ATTENUATION on either dataset: the leaky fit
 *              over-trusts the feature enough that lead-time-matched training
 *              (#81) should pay off.
 *   - MARGINAL — otherwise. At/above MARGINAL_ATTENUATION the effect "barely
 *              moves" and the verdict is provisional (summer); either way,
 *              confirm the real gain via #80's honest backtest before investing
 *              in #81 rather than closing it.
 */
export const recommendation = (
  result: RevisionStudyResult,
): RecommendationResult => {
  const judged = result.datasets.filter(
    (d) =>
      d.productionBandSamples >= MIN_BAND_SAMPLES &&
      d.attenuationIllustration !== null,
  );

  if (judged.length === 0) {
    return {
      verdict: "DEFER",
      reason: `No dataset reached ${String(MIN_BAND_SAMPLES)} revision samples across its sufficient bins with a computable attenuation — insufficient data to decide; re-measure once more (ideally winter) vintages have accrued.`,
    };
  }

  let strongest: DatasetRevisionSummary | null = null;
  for (const d of judged) {
    if (
      strongest === null ||
      (d.attenuationIllustration ?? 1) <
        (strongest.attenuationIllustration ?? 1)
    ) {
      strongest = d;
    }
  }
  if (strongest === null) {
    return { verdict: "DEFER", reason: "No judgeable dataset." };
  }

  const att = strongest.attenuationIllustration;
  const detail = `dataset ${String(strongest.datasetId)}: attenuation ${fmt(att)} (NSR ${fmt(strongest.productionBandNsr)}, ${String(strongest.productionBandSamples)} samples)`;

  if (att !== null && att <= GO_ATTENUATION) {
    return {
      verdict: "GO",
      reason: `${detail} ≤ ${fmt(GO_ATTENUATION, 2)} — the leaky fit over-trusts the forecast enough that lead-time-matched training (#81) should pay off.`,
    };
  }
  if (att !== null && att >= MARGINAL_ATTENUATION) {
    return {
      verdict: "MARGINAL",
      reason: `${detail} ≥ ${fmt(MARGINAL_ATTENUATION, 2)} — the revision effect barely moves the fit; PROVISIONAL (summer). Confirm via #80's vintage-correct backtest; do not close #81 on summer-only data.`,
    };
  }
  return {
    verdict: "MARGINAL",
    reason: `${detail} in (${fmt(GO_ATTENUATION, 2)}, ${fmt(MARGINAL_ATTENUATION, 2)}) — a modest effect; confirm the real gain via #80's vintage-correct backtest before investing in #81 (do not close #81 on summer-only data).`,
  };
};

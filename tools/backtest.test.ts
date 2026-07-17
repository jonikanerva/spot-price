import { describe, expect, it } from "vitest";
import type { FingridRecord, ForecastVintageRecord } from "../src/types.js";
import { quarterKey } from "../src/forecast.js";
import {
  collectInputTimestamps,
  compareOptimism,
  deriveBandsFromBacktest,
  findLeakingInputs,
  forecastAtIssueTime,
  HORIZON_LABELS,
  mae,
  MIN_CALIBRATION_WINDOW_DAYS,
  reconstructIssueTime,
  rmae,
  runBacktest,
  selectVintagesAsOf,
  smape,
  type BacktestData,
  type PricePoint,
} from "./backtest.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;

const actual = (
  startMs: number,
  value: number,
  datasetId: number,
): FingridRecord => ({
  datasetId,
  startTime: new Date(startMs).toISOString(),
  endTime: new Date(startMs + QUARTER_MS).toISOString(),
  value,
});

const vintage = (
  startMs: number,
  value: number,
  datasetId: number,
  issuedMs: number,
): ForecastVintageRecord => ({
  datasetId,
  issuedAt: new Date(issuedMs).toISOString(),
  startTime: new Date(startMs).toISOString(),
  endTime: new Date(startMs + QUARTER_MS).toISOString(),
  value,
});

/**
 * Build a synthetic ~24-day window. Actuals (75/124) are one value per quarter;
 * forecast vintages (245/165) carry a 2-deep issuance ladder: a STALE issuance
 * (−130h, long enough to be knowable at issue time for EVERY target across the
 * whole 72h forecast window — the origin sits ~35h after the wall-clock issue
 * time, so the far window edge needs a >107h lead) and a FRESH one (−2h, only
 * knowable post-origin for future targets → the leaked selection). `staleRevision`
 * scales a mean-zero, per-quarter RANDOM revision on the stale WIND forecast
 * (a constant bias would be absorbed by the ridge fit; genuine noise is the
 * errors-in-variables effect #80 measures). 0 ⇒ a clean, non-revising ladder.
 */
const STALE_LEAD_H = 130;

/** Deterministic mean-≈0 pseudo-noise in [-1, 1] keyed by quarter index. */
const noiseAt = (q: number): number => {
  const s = Math.sin(q * 12.9898) * 43758.5453;
  return (s - Math.floor(s) - 0.5) * 2;
};

const buildData = (staleRevision = 0): BacktestData => {
  const start = Date.parse("2026-02-01T00:00:00.000Z");
  const prices: PricePoint[] = [];
  const wind75: FingridRecord[] = [];
  const cons124: FingridRecord[] = [];
  const wind245: ForecastVintageRecord[] = [];
  const cons165: ForecastVintageRecord[] = [];
  const totalQuarters = 20 * 96;
  for (let q = 0; q < totalQuarters; q++) {
    const ms = start + q * QUARTER_MS;
    const iso = new Date(ms).toISOString();
    const consumption = 8000 + (q % 96) * 15;
    const windMw = 2000 + ((q * 131) % 1800);
    const fiPrice = 0.0012 * (consumption - windMw) + 2;
    prices.push({ area: "FI", start: iso, spotCentsKwh: fiPrice });
    prices.push({ area: "SE1", start: iso, spotCentsKwh: fiPrice - 0.5 });
    prices.push({ area: "SE3", start: iso, spotCentsKwh: fiPrice + 0.3 });
    prices.push({ area: "EE", start: iso, spotCentsKwh: fiPrice + 0.1 });
    cons124.push(actual(ms, consumption, 124));
    wind75.push(actual(ms, windMw, 75));
    // Fresh (−2h) issuance = actual; stale (−130h) wind issuance carries a
    // mean-zero per-quarter revision (only when staleRevision > 0). Consumption
    // is left clean so the wind noise does not cancel in (consumption − wind).
    const staleWind = windMw + staleRevision * noiseAt(q);
    cons165.push(vintage(ms, consumption, 165, ms - 2 * HOUR_MS));
    cons165.push(vintage(ms, consumption, 165, ms - STALE_LEAD_H * HOUR_MS));
    wind245.push(vintage(ms, windMw, 245, ms - 2 * HOUR_MS));
    wind245.push(vintage(ms, staleWind, 245, ms - STALE_LEAD_H * HOUR_MS));
  }
  return {
    prices,
    fingridActualsByDataset: { "75": wind75, "124": cons124 },
    fingridForecastVintagesByDataset: { "245": wind245, "165": cons165 },
  };
};

describe("selectVintagesAsOf", () => {
  it("keeps the freshest issuance STRICTLY before the as-of bound", () => {
    const t = Date.parse("2026-02-10T13:00:00.000Z");
    const vs = [
      vintage(t + DAY_MS, 10, 245, t - HOUR_MS), // 1h before → included
      vintage(t + DAY_MS, 20, 245, t - 3 * HOUR_MS), // 3h before → older
      vintage(t + DAY_MS, 30, 245, t), // exactly at bound → EXCLUDED (strict <)
    ];
    const picked = selectVintagesAsOf(vs, t);
    expect(picked).toHaveLength(1);
    expect(picked[0]?.value).toBe(10); // the freshest admissible one
  });

  it("includes an issuance once the bound moves strictly past it", () => {
    const t = Date.parse("2026-02-10T13:00:00.000Z");
    const vs = [vintage(t + DAY_MS, 30, 245, t)];
    expect(selectVintagesAsOf(vs, t)).toHaveLength(0);
    expect(selectVintagesAsOf(vs, t + 1)).toHaveLength(1);
  });

  it("latest (Infinity) takes the freshest issuance regardless of time", () => {
    const t = Date.parse("2026-02-10T13:00:00.000Z");
    const vs = [
      vintage(t, 10, 245, t - HOUR_MS),
      vintage(t, 99, 245, t + HOUR_MS), // freshest (postdates the target)
    ];
    const picked = selectVintagesAsOf(vs, Number.POSITIVE_INFINITY);
    expect(picked).toHaveLength(1);
    expect(picked[0]?.value).toBe(99);
  });
});

describe("reconstructIssueTime", () => {
  it("only exposes prices through the publication horizon", () => {
    const data = buildData();
    const tMs = Date.parse("2026-02-10T13:00:00.000Z");
    const inputs = reconstructIssueTime(data, tMs);
    expect(inputs.lastPublishedMs).not.toBeNull();
    const horizonEnd = Date.parse("2026-02-12T00:00:00.000Z");
    for (const key of inputs.fiPricesByKey.keys()) {
      expect(new Date(key).getTime()).toBeLessThan(horizonEnd);
    }
  });

  it("keeps actuals strictly before issue time; forecasts stay forward-looking", () => {
    const data = buildData();
    const tMs = Date.parse("2026-02-10T13:00:00.000Z");
    const inputs = reconstructIssueTime(data, tMs);
    for (const r of inputs.fingridActual["124"] ?? []) {
      expect(new Date(r.startTime).getTime()).toBeLessThan(tMs);
    }
    // Selected forecast vintages retain future TARGET timestamps...
    const fcAfter = (inputs.fingridForecast["245"] ?? []).some(
      (r) => new Date(r.startTime).getTime() >= tMs,
    );
    expect(fcAfter).toBe(true);
    // ...but every selected ISSUANCE is strictly before the issue time.
    for (const r of inputs.fingridForecast["245"] ?? []) {
      expect(new Date(r.issuedAt).getTime()).toBeLessThan(tMs);
    }
  });
});

describe("leakage guard (CORRECTNESS-CRITICAL)", () => {
  it("issue-time mode is leak-free across all origins", () => {
    const data = buildData();
    const base = Date.parse("2026-02-05T00:00:00.000Z");
    for (let d = 0; d < 10; d++) {
      for (const hour of [6, 13]) {
        const tMs = base + d * DAY_MS + hour * HOUR_MS;
        const inputs = reconstructIssueTime(data, tMs);
        expect(findLeakingInputs(inputs)).toEqual([]);
        for (const t of collectInputTimestamps(inputs)) {
          if (!t.forwardLookingAllowed) {
            expect(t.ms).toBeLessThan(t.boundaryMs);
          }
        }
      }
    }
    expect(runBacktest(data).leakFree).toBe(true);
  });

  it("latest mode leaks BY DESIGN (a vintage postdates the issue time)", () => {
    const data = buildData();
    // Standalone latest run: the freshest vintage for future targets is issued
    // after the origin, so its issuance entry breaches the tMs boundary.
    expect(runBacktest(data, { asOfMode: "latest" }).leakFree).toBe(false);
  });

  it("flags an injected future-issuance vintage", () => {
    const data = buildData();
    const tMs = Date.parse("2026-02-10T13:00:00.000Z");
    const inputs = reconstructIssueTime(data, tMs);
    const injected = {
      ...inputs,
      fingridForecast: {
        ...inputs.fingridForecast,
        "245": [
          ...(inputs.fingridForecast["245"] ?? []),
          vintage(tMs + DAY_MS, 1234, 245, tMs + HOUR_MS), // issued AFTER tMs
        ],
      },
    };
    const leaks = findLeakingInputs(injected);
    expect(leaks.length).toBeGreaterThan(0);
    expect(leaks.some((l) => l.source === "fingrid_forecast_vintage_245")).toBe(
      true,
    );
  });

  it("still detects a leaking injected FI price", () => {
    const data = buildData();
    const tMs = Date.parse("2026-02-10T13:00:00.000Z");
    const inputs = reconstructIssueTime(data, tMs);
    const leaking = {
      ...inputs,
      fiPricesByKey: new Map(inputs.fiPricesByKey).set(
        quarterKey(inputs.originMs + DAY_MS),
        99,
      ),
    };
    expect(findLeakingInputs(leaking).length).toBeGreaterThan(0);
  });
});

describe("forecastAtIssueTime", () => {
  it("anchors the series one quarter after the last published delivery", () => {
    const data = buildData();
    const tMs = Date.parse("2026-02-10T13:00:00.000Z");
    const inputs = reconstructIssueTime(data, tMs);
    const fc = forecastAtIssueTime(inputs);
    expect(inputs.lastPublishedMs).not.toBeNull();
    if (inputs.lastPublishedMs !== null) {
      expect(fc.seriesStartMs).toBe(inputs.lastPublishedMs + QUARTER_MS);
    }
    expect(fc.predictedByKey.size).toBeGreaterThan(0);
  });
});

describe("metrics", () => {
  it("MAE and sMAPE compute the expected values", () => {
    expect(
      mae([
        [1, 2],
        [3, 3],
      ]),
    ).toBeCloseTo(0.5, 9);
    expect(mae([])).toBeNull();
    expect(
      smape([
        [1, 3],
        [3, 3],
      ]),
    ).toBeCloseTo(0.5, 9);
  });

  it("rMAE is the model/baseline ratio, < 1 when the model wins", () => {
    expect(rmae(0.5, 1)).toBeCloseTo(0.5, 9);
    expect(rmae(0.5, 0)).toBeNull();
    expect(rmae(null, 1)).toBeNull();
  });
});

describe("runBacktest", () => {
  it("scores multiple origins, reports leak-free, and beats naive baselines", () => {
    const data = buildData();
    const summary = runBacktest(data);
    expect(summary.leakFree).toBe(true);
    expect(summary.origins).toBeGreaterThan(5);
    expect(summary.preVintageOrigins).toBe(0); // vintages cover every origin
    expect(summary.scoredOriginsMs.size).toBe(summary.origins);
    expect(summary.modelMae).not.toBeNull();
    expect(summary.rMae.last_week).not.toBeNull();
    if (summary.rMae.last_week !== null) {
      expect(summary.rMae.last_week).toBeLessThan(1);
    }
  });

  it("skips and labels pre-vintage origins", () => {
    // All vintages issued at a fixed mid-window instant → origins before it have
    // no knowable vintage and must be skipped, not scored.
    const start = Date.parse("2026-02-01T00:00:00.000Z");
    const issuedAll = Date.parse("2026-02-20T00:00:00.000Z");
    const prices: PricePoint[] = [];
    const wind75: FingridRecord[] = [];
    const cons124: FingridRecord[] = [];
    const wind245: ForecastVintageRecord[] = [];
    const cons165: ForecastVintageRecord[] = [];
    for (let q = 0; q < 24 * 96; q++) {
      const ms = start + q * QUARTER_MS;
      const consumption = 8000 + (q % 96) * 15;
      const windMw = 2000 + ((q * 131) % 1800);
      prices.push({
        area: "FI",
        start: new Date(ms).toISOString(),
        spotCentsKwh: 0.0012 * (consumption - windMw) + 2,
      });
      cons124.push(actual(ms, consumption, 124));
      wind75.push(actual(ms, windMw, 75));
      cons165.push(vintage(ms, consumption, 165, issuedAll));
      wind245.push(vintage(ms, windMw, 245, issuedAll));
    }
    const summary = runBacktest({
      prices,
      fingridActualsByDataset: { "75": wind75, "124": cons124 },
      fingridForecastVintagesByDataset: { "245": wind245, "165": cons165 },
    });
    expect(summary.preVintageOrigins).toBeGreaterThan(0);
    expect(summary.origins).toBeGreaterThan(0);
  });

  it("reports the scored-origin window", () => {
    const summary = runBacktest(buildData());
    expect(summary.scoredWindowStart).not.toBeNull();
    expect(summary.scoredWindowEnd).not.toBeNull();
    expect(summary.scoredWindowSpanDays).toBeGreaterThan(0);
  });

  it("exposes out-of-sample residuals harvested from the scored pairs", () => {
    const summary = runBacktest(buildData());
    expect(summary.residuals.length).toBeGreaterThan(0);
    for (const r of summary.residuals) {
      expect(r.utcHour).toBeGreaterThanOrEqual(0);
      expect(r.utcHour).toBeLessThanOrEqual(23);
      expect(Number.isFinite(r.predictedRaw)).toBe(true);
      expect(Number.isFinite(r.actualRaw)).toBe(true);
    }
  });
});

describe("deriveBandsFromBacktest (90-day window guard, da cut 1)", () => {
  it("ships DARK when the scored window is under MIN_CALIBRATION_WINDOW_DAYS", () => {
    const { summary, bands, observedCoverage } = deriveBandsFromBacktest(
      buildData(),
      "2026-06-01T00:00:00.000Z",
    );
    expect(summary.scoredWindowSpanDays).toBeLessThan(
      MIN_CALIBRATION_WINDOW_DAYS,
    );
    // Even on clean synthetic data, a sub-season window ships dark by the guard.
    expect(bands.calibrated).toBe(false);
    expect(bands.offsetsByHour.size).toBe(0);
    // The shipped field stays null (contract); the MEASURED coverage is returned
    // separately for the provenance comment (da cut 1) and is a real number.
    expect(bands.observedCoverage).toBeNull();
    expect(observedCoverage).not.toBeNull();
    expect(observedCoverage ?? -1).toBeGreaterThanOrEqual(0);
  });
});

describe("compareOptimism (vintage-leak, issue #80)", () => {
  it("a NON-revising ladder yields ≈ zero optimism", () => {
    const cmp = compareOptimism(buildData(0));
    expect(cmp.scoredOrigins).toBeGreaterThan(0);
    // Both runs score the identical origin set.
    expect(cmp.leaked.scoredOriginsMs.size).toBe(
      cmp.honest.scoredOriginsMs.size,
    );
    expect(cmp.honest.leakFree).toBe(true);
    expect(cmp.leaked.leakFree).toBe(false); // by design
    expect(Math.abs(cmp.deltaMae ?? 0)).toBeLessThan(1e-6);
  });

  it("a REVISING ladder shows the leaked scoreboard is optimistic (lower MAE)", () => {
    const cmp = compareOptimism(buildData(1500)); // stale wind/cons off by 1500 MW
    expect(cmp.scoredOrigins).toBeGreaterThan(0);
    // Leaked feeds the near-actual fresh value → lower error than honest → the
    // leaked scoreboard OVERSTATES the model. delta = leaked − honest < 0.
    expect(cmp.deltaMae).not.toBeNull();
    if (cmp.deltaMae !== null) {
      expect(cmp.deltaMae).toBeLessThan(0);
      expect(Math.abs(cmp.deltaMae)).toBeGreaterThan(1e-3);
    }
  });

  it("reports a per-dataset ladder-depth diagnostic", () => {
    const cmp = compareOptimism(buildData());
    for (const id of ["245", "165"]) {
      const ld = cmp.ladderDiagnostic[id];
      expect(ld).toBeDefined();
      // The 2-deep ladder gives a median depth of 2.
      expect(ld?.medianDepth).toBe(2);
    }
  });
});

describe("per-horizon rank metrics", () => {
  it("populates d+1/d+2/d+3 with pair counts and rank metrics", () => {
    const summary = runBacktest(buildData());
    let totalHorizonPairs = 0;
    for (const h of HORIZON_LABELS) {
      const m = summary.byHorizon[h];
      expect(m.pairs).toBeGreaterThan(0);
      totalHorizonPairs += m.pairs;
      expect(m.spearman).not.toBeNull();
      expect(m.precisionAtNCheap).not.toBeNull();
      expect(m.precisionAtNPeak).not.toBeNull();
    }
    expect(totalHorizonPairs).toBe(summary.residuals.length);
  });
});

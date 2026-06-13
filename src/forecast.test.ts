import { describe, expect, it } from "vitest";
import type { FingridRecord } from "./types.js";
import {
  alignSeries,
  applyHourBias,
  bucketRecords,
  buildForecast,
  buildPredictedSeries,
  expandHourlyToQuarters,
  extendWithLastWeek,
  fitHourBias,
  fitLinear,
  priceFloorFromHistory,
  quarterFloorUtc,
  quarterKey,
} from "./forecast.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const WEEK_MS = 7 * DAY_MS;

const rec = (startMs: number, value: number, datasetId = 1): FingridRecord => ({
  datasetId,
  startTime: new Date(startMs).toISOString(),
  endTime: new Date(startMs + QUARTER_MS).toISOString(),
  value,
});

// A fixed UTC anchor that is exactly on a quarter and hour boundary.
const ANCHOR = Date.parse("2026-03-01T00:00:00.000Z");

describe("quarterFloorUtc / quarterKey", () => {
  it("floors to the 15-minute boundary", () => {
    const ms = Date.parse("2026-03-01T10:07:33.500Z");
    expect(new Date(quarterFloorUtc(ms)).toISOString()).toBe(
      "2026-03-01T10:00:00.000Z",
    );
    expect(quarterKey(Date.parse("2026-03-01T10:59:59.999Z"))).toBe(
      "2026-03-01T10:45:00.000Z",
    );
  });

  it("is idempotent on a boundary", () => {
    expect(quarterFloorUtc(ANCHOR)).toBe(ANCHOR);
  });
});

describe("bucketRecords", () => {
  it("averages multiple samples in the same quarter", () => {
    const out = bucketRecords([rec(ANCHOR, 10), rec(ANCHOR + 60_000, 20)]);
    expect(out.get(quarterKey(ANCHOR))).toBe(15);
  });

  it("skips malformed records (NaN value, bad time)", () => {
    const bad: FingridRecord = {
      datasetId: 1,
      startTime: "not-a-date",
      endTime: "x",
      value: 5,
    };
    const out = bucketRecords([rec(ANCHOR, 10), bad, rec(ANCHOR, Number.NaN)]);
    expect(out.size).toBe(1);
    expect(out.get(quarterKey(ANCHOR))).toBe(10);
  });
});

describe("expandHourlyToQuarters", () => {
  it("fills :15/:30/:45 from a top-of-hour-only value", () => {
    const out = expandHourlyToQuarters([rec(ANCHOR, 100)]);
    expect(out.size).toBe(4);
    for (let q = 0; q < 4; q++) {
      expect(out.get(quarterKey(ANCHOR + q * QUARTER_MS))).toBe(100);
    }
  });

  it("preserves distinct 15-min values without overwriting", () => {
    const out = expandHourlyToQuarters([
      rec(ANCHOR, 100),
      rec(ANCHOR + QUARTER_MS, 200),
    ]);
    expect(out.get(quarterKey(ANCHOR))).toBe(100);
    expect(out.get(quarterKey(ANCHOR + QUARTER_MS))).toBe(200);
    // :30 and :45 fill from :00 since they are absent
    expect(out.get(quarterKey(ANCHOR + 2 * QUARTER_MS))).toBe(100);
  });
});

describe("extendWithLastWeek", () => {
  it("fills past the forecast horizon from the same weekday/quarter", () => {
    const forecast = new Map<string, number>([[quarterKey(ANCHOR), 50]]);
    // actual one week before the quarter we want to fill (ANCHOR + 1 quarter)
    const target = ANCHOR + QUARTER_MS;
    const actual = new Map<string, number>([
      [quarterKey(target - WEEK_MS), 77],
    ]);
    const out = extendWithLastWeek(
      forecast,
      actual,
      ANCHOR + 2 * QUARTER_MS,
      1,
    );
    expect(out.get(quarterKey(target))).toBe(77);
  });

  it("averages over multiple weeks for wind", () => {
    const forecast = new Map<string, number>([[quarterKey(ANCHOR), 50]]);
    const target = ANCHOR + QUARTER_MS;
    const actual = new Map<string, number>([
      [quarterKey(target - WEEK_MS), 60],
      [quarterKey(target - 2 * WEEK_MS), 80],
    ]);
    const out = extendWithLastWeek(
      forecast,
      actual,
      ANCHOR + 2 * QUARTER_MS,
      2,
    );
    expect(out.get(quarterKey(target))).toBe(70);
  });

  it("leaves quarters with no history unfilled", () => {
    const forecast = new Map<string, number>([[quarterKey(ANCHOR), 50]]);
    const out = extendWithLastWeek(
      forecast,
      new Map(),
      ANCHOR + 4 * QUARTER_MS,
      1,
    );
    expect(out.size).toBe(1);
  });

  it("returns empty for empty forecast", () => {
    const out = extendWithLastWeek(new Map(), new Map(), ANCHOR, 1);
    expect(out.size).toBe(0);
  });
});

describe("fitLinear", () => {
  it("recovers a known slope and intercept", () => {
    // y = 2x + 1
    const xs = [0, 1, 2, 3, 4];
    const ys = xs.map((x) => 2 * x + 1);
    const { slope, intercept } = fitLinear(xs, ys);
    expect(slope).toBeCloseTo(2, 9);
    expect(intercept).toBeCloseTo(1, 9);
  });

  it("throws with fewer than 2 points", () => {
    expect(() => fitLinear([1], [1])).toThrow();
  });

  it("throws on zero variance in x", () => {
    expect(() => fitLinear([5, 5, 5], [1, 2, 3])).toThrow();
  });
});

describe("alignSeries", () => {
  it("aligns on common keys in chronological order", () => {
    const price = new Map<string, number>([
      [quarterKey(ANCHOR), 10],
      [quarterKey(ANCHOR + QUARTER_MS), 20],
      [quarterKey(ANCHOR + 2 * QUARTER_MS), 30],
    ]);
    const residual = new Map<string, number>([
      [quarterKey(ANCHOR + QUARTER_MS), 200],
      [quarterKey(ANCHOR + 2 * QUARTER_MS), 300],
    ]);
    const { xs, ys } = alignSeries(price, residual);
    expect(xs).toEqual([200, 300]);
    expect(ys).toEqual([20, 30]);
  });
});

describe("fitHourBias / applyHourBias", () => {
  it("computes mean residual per UTC hour and applies it", () => {
    const k0 = quarterKey(ANCHOR); // 00:00 UTC
    const k1 = quarterKey(ANCHOR + HOUR_MS); // 01:00 UTC
    const actual = new Map<string, number>([
      [k0, 12],
      [k1, 5],
    ]);
    const predicted = new Map<string, number>([
      [k0, 10],
      [k1, 8],
    ]);
    const { biasByHour, globalBias } = fitHourBias(actual, predicted);
    expect(biasByHour.get(0)).toBe(2); // 12 - 10
    expect(biasByHour.get(1)).toBe(-3); // 5 - 8
    expect(globalBias).toBeCloseTo(-0.5, 9);

    const corrected = applyHourBias(predicted, biasByHour, globalBias);
    expect(corrected.get(k0)).toBe(12);
    expect(corrected.get(k1)).toBe(5);
  });

  it("falls back to global bias for unseen hours", () => {
    const biasByHour = new Map<number, number>([[0, 2]]);
    const k5 = quarterKey(ANCHOR + 5 * HOUR_MS);
    const predicted = new Map<string, number>([[k5, 10]]);
    const corrected = applyHourBias(predicted, biasByHour, 1.5);
    expect(corrected.get(k5)).toBe(11.5);
  });
});

describe("priceFloorFromHistory", () => {
  it("returns the percentile of hourly minima", () => {
    // 10 hours, minima 1..10; 5th percentile index = trunc(10*5/100)-1 = -? -> max(0, -1) = 0
    const history = new Map<string, number>();
    for (let h = 0; h < 10; h++) {
      // two quarters per hour, the smaller one is the minimum
      history.set(quarterKey(ANCHOR + h * HOUR_MS), h + 1);
      history.set(quarterKey(ANCHOR + h * HOUR_MS + QUARTER_MS), h + 100);
    }
    const floor = priceFloorFromHistory(history, 5);
    expect(floor).toBe(1);
  });

  it("returns null with no history", () => {
    expect(priceFloorFromHistory(new Map())).toBeNull();
  });
});

describe("buildPredictedSeries", () => {
  it("produces exactly numQuarters entries, forward-filling gaps", () => {
    const predicted = new Map<string, number>([
      [quarterKey(ANCHOR), 5],
      // ANCHOR+1 missing -> forward fill 5
      [quarterKey(ANCHOR + 2 * QUARTER_MS), 7],
    ]);
    const built = buildPredictedSeries(predicted, ANCHOR, 3);
    expect(built.series).toHaveLength(3);
    expect(built.series.map((p) => p.estimatedSpotCentsKwh)).toEqual([5, 5, 7]);
    expect(built.filledQuarters).toBe(1);
    expect(built.zeroSeededQuarters).toBe(0);
  });

  it("seeds leading gaps by looking ahead", () => {
    const predicted = new Map<string, number>([
      [quarterKey(ANCHOR + 2 * QUARTER_MS), 9],
    ]);
    const built = buildPredictedSeries(predicted, ANCHOR, 3);
    expect(built.series.map((p) => p.estimatedSpotCentsKwh)).toEqual([9, 9, 9]);
    // leading two are seeded (counted as filled), none zero-seeded
    expect(built.zeroSeededQuarters).toBe(0);
  });

  it("zero-seeds a hard outage when nothing is predicted", () => {
    const built = buildPredictedSeries(new Map(), ANCHOR, 2);
    expect(built.series.map((p) => p.estimatedSpotCentsKwh)).toEqual([0, 0]);
    expect(built.zeroSeededQuarters).toBe(2);
  });
});

describe("buildForecast (integration of the pure pipeline)", () => {
  // Build a synthetic month where spot price = trueSlope * (consumption - wind)
  // + trueIntercept, so the ridge model (residual is one of its features) can
  // recover the relationship well.
  const trueSlope = 0.001;
  const trueIntercept = 3;
  const priceFor = (consumption: number, windMw: number): number =>
    trueSlope * (consumption - windMw) + trueIntercept;

  const buildSeries = (): {
    spot: Map<string, number>;
    cons: FingridRecord[];
    wind: FingridRecord[];
  } => {
    const spot = new Map<string, number>();
    const cons: FingridRecord[] = [];
    const wind: FingridRecord[] = [];
    for (let q = 0; q < 30 * 96; q++) {
      const ms = ANCHOR + q * QUARTER_MS;
      const consumption = 8000 + (q % 96) * 20; // daily ramp
      const windMw = 2000 + ((q * 137) % 1500); // pseudo-noise
      spot.set(quarterKey(ms), priceFor(consumption, windMw));
      cons.push(rec(ms, consumption, 124));
      wind.push(rec(ms, windMw, 245));
    }
    return { spot, cons, wind };
  };

  it("fits the model and predicts the future window without falling back", () => {
    const { spot, cons, wind } = buildSeries();
    const seriesStartMs = ANCHOR + 30 * 96 * QUARTER_MS;
    const seriesEndMs = seriesStartMs + 3 * DAY_MS;

    // Provide future consumption/wind so the forecast horizon is covered, and
    // record the true price for each future quarter to score against.
    const futureCons: FingridRecord[] = [];
    const futureWind: FingridRecord[] = [];
    const truthByKey = new Map<string, number>();
    for (let q = 0; q < 3 * 96; q++) {
      const ms = seriesStartMs + q * QUARTER_MS;
      const consumption = 8500 + (q % 96) * 18;
      const windMw = 2200 + ((q * 113) % 1400);
      futureCons.push(rec(ms, consumption, 124));
      futureWind.push(rec(ms, windMw, 245));
      truthByKey.set(quarterKey(ms), priceFor(consumption, windMw));
    }

    const result = buildForecast(
      {
        spotPricesByKey: spot,
        windForecast: [...wind, ...futureWind],
        windActual: wind,
        consumptionForecast: [...cons, ...futureCons],
        consumptionActual: cons,
        seriesStartMs,
        seriesEndMs,
      },
      { applyTimeBias: false },
    );

    expect(result.diagnostics.fitUsedDefault).toBe(false);
    expect(result.diagnostics.fitSamples).toBeGreaterThanOrEqual(24);
    expect(result.diagnostics.featureCount).toBeGreaterThan(0);
    // Exactly 3 days of quarters
    expect(result.series).toHaveLength(3 * 96);
    expect(result.diagnostics.zeroSeededQuarters).toBe(0);
    // The recovered estimate tracks the synthetic truth closely (low MAE).
    let sumAbs = 0;
    for (const point of result.series) {
      const truth = truthByKey.get(point.start) ?? 0;
      sumAbs += Math.abs(point.estimatedSpotCentsKwh - truth);
    }
    const meanAbsError = sumAbs / result.series.length;
    expect(meanAbsError).toBeLessThan(0.5);
  });

  it("falls back when there is insufficient price history to fit", () => {
    const seriesStartMs = ANCHOR + 96 * QUARTER_MS;
    const seriesEndMs = seriesStartMs + DAY_MS;
    const result = buildForecast(
      {
        spotPricesByKey: new Map([[quarterKey(ANCHOR), 5]]),
        windForecast: [],
        windActual: [],
        consumptionForecast: [],
        consumptionActual: [],
        seriesStartMs,
        seriesEndMs,
      },
      { applyTimeBias: false },
    );
    expect(result.diagnostics.fitUsedDefault).toBe(true);
    expect(result.diagnostics.fitSamples).toBeLessThan(24);
    // The model returns its constant fallback for every quarter — a single
    // distinct value, never zero-seeded (there IS a prediction for each quarter).
    expect(result.diagnostics.zeroSeededQuarters).toBe(0);
    const distinct = new Set(result.series.map((p) => p.estimatedSpotCentsKwh));
    expect(distinct.size).toBe(1);
  });

  it("applies the floor clip and counts clipped quarters", () => {
    const seriesStartMs = ANCHOR;
    const seriesEndMs = seriesStartMs + QUARTER_MS * 2;
    // No history → model falls back to its constant (~3.0); floor above it clips
    // every quarter.
    const cons = [rec(ANCHOR, 100, 124), rec(ANCHOR + QUARTER_MS, 100, 124)];
    const result = buildForecast(
      {
        spotPricesByKey: new Map(),
        windForecast: [],
        windActual: [],
        consumptionForecast: cons,
        consumptionActual: [],
        seriesStartMs,
        seriesEndMs,
      },
      { applyTimeBias: false, floor: 5 },
    );
    expect(result.diagnostics.predictionFloor).toBe(5);
    expect(result.diagnostics.floorClippedQuarters).toBe(2);
    for (const point of result.series) {
      expect(point.estimatedSpotCentsKwh).toBeGreaterThanOrEqual(5);
    }
  });
});

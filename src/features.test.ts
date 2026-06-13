import { describe, expect, it } from "vitest";
import {
  buildFeatureVector,
  buildTrainingMatrix,
  FEATURE_NAMES,
  NEIGHBOR_AREAS,
  TRAINING_HISTORY_DAYS,
  type FeatureContext,
} from "./features.js";
import { quarterKey } from "./forecast.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const WEEK_MS = 7 * DAY_MS;

// 2026-03-04 is a Wednesday (getUTCDay() === 3); 12:00 UTC, on a quarter/hour
// boundary. Fixed so calendar features are deterministic.
const WED_NOON = Date.parse("2026-03-04T12:00:00.000Z");

const idx = (name: string): number => {
  const i = FEATURE_NAMES.indexOf(name);
  if (i < 0) {
    throw new Error(`unknown feature ${name}`);
  }
  return i;
};

const emptyCtx = (): FeatureContext => ({
  fiPricesByKey: new Map(),
  neighborPricesByArea: new Map(),
  windByKey: new Map(),
  consumptionByKey: new Map(),
});

describe("buildFeatureVector — shape & order", () => {
  it("produces a vector with exactly FEATURE_NAMES.length entries", () => {
    const v = buildFeatureVector(WED_NOON, emptyCtx());
    expect(v).toHaveLength(FEATURE_NAMES.length);
  });

  it("computes the residual and interaction term from wind/consumption", () => {
    const key = quarterKey(WED_NOON);
    const ctx: FeatureContext = {
      ...emptyCtx(),
      windByKey: new Map([[key, 2000]]),
      consumptionByKey: new Map([[key, 9000]]),
    };
    const v = buildFeatureVector(WED_NOON, ctx);
    expect(v[idx("wind_mw")]).toBe(2000);
    expect(v[idx("consumption_mw")]).toBe(9000);
    expect(v[idx("residual_mw")]).toBe(7000);
    expect(v[idx("wind_x_consumption")]).toBe(2000 * 9000);
  });
});

describe("buildFeatureVector — calendar features (UTC only)", () => {
  it("encodes UTC hour and day-of-week cyclically, weekend flag off on Wednesday", () => {
    const v = buildFeatureVector(WED_NOON, emptyCtx());
    // hour = 12 → sin(π) ≈ 0, cos(π) = -1.
    expect(v[idx("hour_sin")]).toBeCloseTo(0, 9);
    expect(v[idx("hour_cos")]).toBeCloseTo(-1, 9);
    // dow = 3 (Wednesday).
    expect(v[idx("dow_sin")]).toBeCloseTo(Math.sin((2 * Math.PI * 3) / 7), 9);
    expect(v[idx("dow_cos")]).toBeCloseTo(Math.cos((2 * Math.PI * 3) / 7), 9);
    expect(v[idx("is_weekend")]).toBe(0);
  });

  it("sets the weekend flag on Saturday and Sunday", () => {
    const sat = Date.parse("2026-03-07T08:00:00.000Z"); // Saturday
    const sun = Date.parse("2026-03-08T08:00:00.000Z"); // Sunday
    expect(buildFeatureVector(sat, emptyCtx())[idx("is_weekend")]).toBe(1);
    expect(buildFeatureVector(sun, emptyCtx())[idx("is_weekend")]).toBe(1);
  });
});

describe("buildFeatureVector — FI price lags", () => {
  it("reads the same-quarter 1d / 7d lags and previous-day stats", () => {
    const fi = new Map<string, number>();
    fi.set(quarterKey(WED_NOON - WEEK_MS), 22); // 7d ago
    // Previous UTC day quarters → min 1, max 11, mean 5. The 12:00 quarter (the
    // 1d-ago lag for a 12:00 target) carries 11, so it doubles as the prev-day
    // max and is the value the 1d lag must read.
    const prevDayStart = Math.floor(WED_NOON / DAY_MS) * DAY_MS - DAY_MS;
    fi.set(quarterKey(prevDayStart + 4 * HOUR_MS), 1);
    fi.set(quarterKey(prevDayStart + 8 * HOUR_MS), 3);
    fi.set(quarterKey(prevDayStart + 12 * HOUR_MS), 11); // == WED_NOON - DAY_MS
    const ctx: FeatureContext = { ...emptyCtx(), fiPricesByKey: fi };
    const v = buildFeatureVector(WED_NOON, ctx);
    expect(v[idx("fi_same_quarter_1d")]).toBe(11);
    expect(v[idx("fi_same_quarter_7d")]).toBe(22);
    expect(v[idx("fi_prev_day_min")]).toBe(1);
    expect(v[idx("fi_prev_day_max")]).toBe(11);
    expect(v[idx("fi_prev_day_mean")]).toBeCloseTo(5, 9);
  });
});

describe("buildFeatureVector — neighbor lags & neutral fill", () => {
  it("reads neighbor 1d / 7d lags when present", () => {
    const se1 = new Map<string, number>([
      [quarterKey(WED_NOON - DAY_MS), 7],
      [quarterKey(WED_NOON - WEEK_MS), 9],
    ]);
    const ctx: FeatureContext = {
      ...emptyCtx(),
      neighborPricesByArea: new Map([["SE1", se1]]),
    };
    const v = buildFeatureVector(WED_NOON, ctx);
    expect(v[idx("se1_1d")]).toBe(7);
    expect(v[idx("se1_7d")]).toBe(9);
  });

  it("neutral-fills missing neighbors with the FI mean (never errors)", () => {
    const fi = new Map<string, number>([
      [quarterKey(WED_NOON - 2 * DAY_MS), 4],
      [quarterKey(WED_NOON - 3 * DAY_MS), 6],
    ]); // mean = 5
    const ctx: FeatureContext = {
      ...emptyCtx(),
      fiPricesByKey: fi,
      neighborPricesByArea: new Map(), // all neighbors absent
    };
    const v = buildFeatureVector(WED_NOON, ctx);
    for (const area of NEIGHBOR_AREAS) {
      const lower = area.toLowerCase();
      expect(v[idx(`${lower}_1d`)]).toBe(5);
      expect(v[idx(`${lower}_7d`)]).toBe(5);
    }
  });

  it("neutral-fills with 0 when there is no FI history at all", () => {
    const v = buildFeatureVector(WED_NOON, emptyCtx());
    expect(v[idx("se1_1d")]).toBe(0);
    expect(v[idx("fi_latest_published")]).toBe(0);
  });
});

describe("buildTrainingMatrix", () => {
  it("emits one row per in-window FI quarter, each with FEATURE_NAMES.length columns", () => {
    const origin = WED_NOON;
    const fi = new Map<string, number>();
    // 200 quarters ending just before the origin, all inside the window.
    for (let i = 1; i <= 200; i++) {
      fi.set(quarterKey(origin - i * QUARTER_MS), i % 10);
    }
    const ctx: FeatureContext = { ...emptyCtx(), fiPricesByKey: fi };
    const built = buildTrainingMatrix(origin, ctx);
    expect(built.features.featureNames).toEqual(FEATURE_NAMES);
    expect(built.targets).toHaveLength(200);
    expect(built.features.rows).toHaveLength(200);
    for (const row of built.features.rows) {
      expect(row).toHaveLength(FEATURE_NAMES.length);
    }
  });

  it("excludes quarters at or after the origin and older than the training window", () => {
    const origin = WED_NOON;
    const fi = new Map<string, number>();
    fi.set(quarterKey(origin), 99); // at origin → excluded (would be the target)
    fi.set(quarterKey(origin + QUARTER_MS), 99); // future → excluded
    fi.set(quarterKey(origin - QUARTER_MS), 5); // in window → included
    fi.set(quarterKey(origin - (TRAINING_HISTORY_DAYS + 1) * DAY_MS), 7); // too old → excluded
    const ctx: FeatureContext = { ...emptyCtx(), fiPricesByKey: fi };
    const built = buildTrainingMatrix(origin, ctx);
    expect(built.targets).toEqual([5]);
  });

  it("a training row matches the prediction vector for the same quarter", () => {
    // Column semantics must be identical between training and prediction.
    const origin = WED_NOON;
    const targetMs = origin - QUARTER_MS;
    const key = quarterKey(targetMs);
    const fi = new Map<string, number>([[key, 3]]);
    const ctx: FeatureContext = {
      ...emptyCtx(),
      fiPricesByKey: fi,
      windByKey: new Map([[key, 1500]]),
      consumptionByKey: new Map([[key, 8000]]),
    };
    const built = buildTrainingMatrix(origin, ctx);
    const predVec = buildFeatureVector(targetMs, ctx);
    expect(built.features.rows[0]).toEqual(predVec);
  });
});

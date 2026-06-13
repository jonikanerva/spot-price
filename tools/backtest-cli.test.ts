import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  assembleBacktestData,
  DEFAULT_WINDOW_DAYS,
  isThinData,
  MIN_SCOREABLE_ORIGINS,
  parseWindowDays,
  toFixtureJson,
} from "./backtest-cli.js";
import { loadFixture, runBacktest } from "./backtest.js";
import { eurMwhToCentsKwh } from "../src/nordpool.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";
import type { FingridRecord, HourlyPrice } from "../src/types.js";

const QUARTER_MS = 15 * 60 * 1000;

const price = (area: string, ms: number, eurMwh: number): HourlyPrice => ({
  deliveryStart: new Date(ms).toISOString(),
  deliveryEnd: new Date(ms + QUARTER_MS).toISOString(),
  priceEurMwh: eurMwh,
  area,
});

const fgRecord = (
  ms: number,
  value: number,
  datasetId: number,
): FingridRecord => ({
  datasetId,
  startTime: new Date(ms).toISOString(),
  endTime: new Date(ms + QUARTER_MS).toISOString(),
  value,
});

/**
 * Build a synthetic month of FI + neighbour DB rows and the four Fingrid
 * datasets, returned in the shapes the stores produce: prices grouped by area,
 * Fingrid keyed by string dataset id.
 */
const buildDbShapedInput = (): {
  pricesByArea: Map<string, HourlyPrice[]>;
  fingridByDataset: Record<string, FingridRecord[]>;
} => {
  const start = Date.parse("2026-02-01T00:00:00.000Z");
  const fi: HourlyPrice[] = [];
  const se1: HourlyPrice[] = [];
  const se3: HourlyPrice[] = [];
  const ee: HourlyPrice[] = [];
  const wind245: FingridRecord[] = [];
  const wind75: FingridRecord[] = [];
  const cons165: FingridRecord[] = [];
  const cons124: FingridRecord[] = [];
  const totalQuarters = 40 * 96;
  for (let q = 0; q < totalQuarters; q++) {
    const ms = start + q * QUARTER_MS;
    const consumption = 8000 + (q % 96) * 15;
    const windMw = 2000 + ((q * 131) % 1800);
    // EUR/MWh with real structure; conversion happens in assembleBacktestData.
    const eur = 0.12 * (consumption - windMw) + 200;
    fi.push(price("FI", ms, eur));
    se1.push(price("SE1", ms, eur - 5));
    se3.push(price("SE3", ms, eur + 3));
    ee.push(price("EE", ms, eur + 1));
    cons124.push(fgRecord(ms, consumption, DATASET_CONSUMPTION_ACTUAL));
    wind75.push(fgRecord(ms, windMw, DATASET_WIND_ACTUAL));
    cons165.push(fgRecord(ms, consumption, DATASET_CONSUMPTION_FORECAST));
    wind245.push(fgRecord(ms, windMw, DATASET_WIND_FORECAST));
  }
  return {
    pricesByArea: new Map([
      ["FI", fi],
      ["SE1", se1],
      ["SE3", se3],
      ["EE", ee],
    ]),
    fingridByDataset: {
      "245": wind245,
      "75": wind75,
      "165": cons165,
      "124": cons124,
    },
  };
};

describe("assembleBacktestData — parity with loadFixture", () => {
  it("produces a BacktestData deep-equal to loadFixture from the equivalent fixture", () => {
    const { pricesByArea, fingridByDataset } = buildDbShapedInput();
    const assembled = assembleBacktestData(pricesByArea, fingridByDataset);

    // Equivalent fixture JSON: prices already post-conversion (c/kWh) so
    // loadFixture reads them as-is, exactly like an --export → --data replay.
    const tmp = mkdtempSync(path.join(tmpdir(), "bt-parity-"));
    try {
      writeFileSync(
        path.join(tmp, "fixture.json"),
        toFixtureJson(assembled),
        "utf-8",
      );
      const loaded = loadFixture(tmp);
      expect(loaded).toEqual(assembled);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("converts EUR/MWh → c/kWh exactly once via eurMwhToCentsKwh", () => {
    const pricesByArea = new Map<string, HourlyPrice[]>([
      ["FI", [price("FI", Date.parse("2026-02-01T00:00:00.000Z"), 312.5)]],
    ]);
    const assembled = assembleBacktestData(pricesByArea, {});
    expect(assembled.prices[0]?.spotCentsKwh).toBe(eurMwhToCentsKwh(312.5));
  });

  it("passes Fingrid records through raw, keyed by string dataset id", () => {
    const { fingridByDataset } = buildDbShapedInput();
    const assembled = assembleBacktestData(new Map(), fingridByDataset);
    expect(Object.keys(assembled.fingridByDataset).sort()).toEqual([
      "124",
      "165",
      "245",
      "75",
    ]);
    // Raw pass-through: same record count, no bucketing.
    expect(assembled.fingridByDataset["124"]).toHaveLength(
      fingridByDataset["124"]?.length ?? -1,
    );
  });
});

describe("export round-trip — assemble vs fixture replay (da Obj.2)", () => {
  it("runBacktest(assembled) deep-equals runBacktest(loadFixture(export))", () => {
    const { pricesByArea, fingridByDataset } = buildDbShapedInput();
    const assembled = assembleBacktestData(pricesByArea, fingridByDataset);
    const summaryA = runBacktest(assembled);

    const tmp = mkdtempSync(path.join(tmpdir(), "bt-roundtrip-"));
    try {
      writeFileSync(
        path.join(tmp, "fixture.json"),
        toFixtureJson(assembled),
        "utf-8",
      );
      const replayed = loadFixture(tmp);
      const summaryB = runBacktest(replayed);

      // Catches double-conversion, Date-vs-ISO, string-vs-number keys, and
      // any accidental pre-bucketing.
      expect(summaryB.origins).toBe(summaryA.origins);
      expect(summaryB.fallbackOrigins).toBe(summaryA.fallbackOrigins);
      expect(summaryB.leakFree).toBe(summaryA.leakFree);
      expect(summaryB.modelMae).toBe(summaryA.modelMae);
      expect(summaryB.modelSmape).toBe(summaryA.modelSmape);
      expect(summaryB.rMae).toEqual(summaryA.rMae);
      expect(summaryB.baselineMae).toEqual(summaryA.baselineMae);
      expect(summaryB.residuals).toEqual(summaryA.residuals);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("a real DB window of several days produces a scoreable, leak-free run", () => {
    const { pricesByArea, fingridByDataset } = buildDbShapedInput();
    const assembled = assembleBacktestData(pricesByArea, fingridByDataset);
    const summary = runBacktest(assembled);
    expect(summary.leakFree).toBe(true);
    expect(summary.origins).toBeGreaterThan(0);
  });
});

describe("parseWindowDays", () => {
  it("defaults to DEFAULT_WINDOW_DAYS when absent", () => {
    expect(parseWindowDays(undefined)).toBe(DEFAULT_WINDOW_DAYS);
  });

  it("accepts a positive integer", () => {
    expect(parseWindowDays("120")).toBe(120);
  });

  it("rejects non-integers, zero, negatives, and junk (no unchecked parseInt)", () => {
    expect(() => parseWindowDays("0")).toThrow();
    expect(() => parseWindowDays("-5")).toThrow();
    expect(() => parseWindowDays("30.5")).toThrow();
    expect(() => parseWindowDays("abc")).toThrow();
    expect(() => parseWindowDays("30days")).toThrow();
  });
});

describe("thin-data guard", () => {
  it("flags fewer than MIN_SCOREABLE_ORIGINS as thin (warn + non-zero exit)", () => {
    expect(isThinData(MIN_SCOREABLE_ORIGINS - 1)).toBe(true);
    expect(isThinData(0)).toBe(true);
  });

  it("treats at/above the threshold as a verdict", () => {
    expect(isThinData(MIN_SCOREABLE_ORIGINS)).toBe(false);
    expect(isThinData(MIN_SCOREABLE_ORIGINS + 50)).toBe(false);
  });
});

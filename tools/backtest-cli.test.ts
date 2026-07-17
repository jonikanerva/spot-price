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
import { compareOptimism, loadFixture, runBacktest } from "./backtest.js";
import { eurMwhToCentsKwh } from "../src/nordpool.js";
import type {
  FingridRecord,
  ForecastVintageRecord,
  HourlyPrice,
} from "../src/types.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;

const price = (area: string, ms: number, eurMwh: number): HourlyPrice => ({
  deliveryStart: new Date(ms).toISOString(),
  deliveryEnd: new Date(ms + QUARTER_MS).toISOString(),
  priceEurMwh: eurMwh,
  area,
});

const actual = (
  ms: number,
  value: number,
  datasetId: number,
): FingridRecord => ({
  datasetId,
  startTime: new Date(ms).toISOString(),
  endTime: new Date(ms + QUARTER_MS).toISOString(),
  value,
});

const vintage = (
  ms: number,
  value: number,
  datasetId: number,
  issuedMs: number,
): ForecastVintageRecord => ({
  datasetId,
  issuedAt: new Date(issuedMs).toISOString(),
  startTime: new Date(ms).toISOString(),
  endTime: new Date(ms + QUARTER_MS).toISOString(),
  value,
});

/**
 * DB-shaped inputs: prices grouped by area, actuals (75/124) and forecast
 * VINTAGES (245/165) keyed by string dataset id, with a 2-deep issuance ladder.
 */
const buildDbShaped = (): {
  pricesByArea: Map<string, HourlyPrice[]>;
  fingridActualsByDataset: Record<string, FingridRecord[]>;
  fingridForecastVintagesByDataset: Record<string, ForecastVintageRecord[]>;
} => {
  const start = Date.parse("2026-02-01T00:00:00.000Z");
  const fi: HourlyPrice[] = [];
  const se1: HourlyPrice[] = [];
  const se3: HourlyPrice[] = [];
  const ee: HourlyPrice[] = [];
  const wind75: FingridRecord[] = [];
  const cons124: FingridRecord[] = [];
  const wind245: ForecastVintageRecord[] = [];
  const cons165: ForecastVintageRecord[] = [];
  const totalQuarters = 14 * 96;
  for (let q = 0; q < totalQuarters; q++) {
    const ms = start + q * QUARTER_MS;
    const consumption = 8000 + (q % 96) * 15;
    const windMw = 2000 + ((q * 131) % 1800);
    const eur = 0.12 * (consumption - windMw) + 200;
    fi.push(price("FI", ms, eur));
    se1.push(price("SE1", ms, eur - 5));
    se3.push(price("SE3", ms, eur + 3));
    ee.push(price("EE", ms, eur + 1));
    cons124.push(actual(ms, consumption, 124));
    wind75.push(actual(ms, windMw, 75));
    cons165.push(vintage(ms, consumption, 165, ms - 2 * HOUR_MS));
    cons165.push(vintage(ms, consumption + 300, 165, ms - 80 * HOUR_MS));
    wind245.push(vintage(ms, windMw, 245, ms - 2 * HOUR_MS));
    wind245.push(vintage(ms, windMw + 300, 245, ms - 80 * HOUR_MS));
  }
  return {
    pricesByArea: new Map([
      ["FI", fi],
      ["SE1", se1],
      ["SE3", se3],
      ["EE", ee],
    ]),
    fingridActualsByDataset: { "75": wind75, "124": cons124 },
    fingridForecastVintagesByDataset: { "245": wind245, "165": cons165 },
  };
};

describe("assembleBacktestData — parity with loadFixture", () => {
  it("produces a BacktestData deep-equal to loadFixture from the equivalent fixture", () => {
    const {
      pricesByArea,
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    } = buildDbShaped();
    const assembled = assembleBacktestData(
      pricesByArea,
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    );

    const tmp = mkdtempSync(path.join(tmpdir(), "bt-parity-"));
    try {
      const file = path.join(tmp, "fixture.json");
      writeFileSync(file, toFixtureJson(assembled), "utf-8");
      const loaded = loadFixture(file);
      expect(loaded).toEqual(assembled);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("converts EUR/MWh → c/kWh exactly once via eurMwhToCentsKwh", () => {
    const pricesByArea = new Map<string, HourlyPrice[]>([
      ["FI", [price("FI", Date.parse("2026-02-01T00:00:00.000Z"), 312.5)]],
    ]);
    const assembled = assembleBacktestData(pricesByArea, {}, {});
    expect(assembled.prices[0]?.spotCentsKwh).toBe(eurMwhToCentsKwh(312.5));
  });

  it("keeps actuals and forecast vintages in their split maps, keyed by string id", () => {
    const { fingridActualsByDataset, fingridForecastVintagesByDataset } =
      buildDbShaped();
    const assembled = assembleBacktestData(
      new Map(),
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    );
    expect(Object.keys(assembled.fingridActualsByDataset).sort()).toEqual([
      "124",
      "75",
    ]);
    expect(
      Object.keys(assembled.fingridForecastVintagesByDataset).sort(),
    ).toEqual(["165", "245"]);
    // Vintages carry issuedAt (the whole point of #80).
    expect(
      assembled.fingridForecastVintagesByDataset["245"]?.[0]?.issuedAt,
    ).toBeTypeOf("string");
  });
});

describe("export round-trip and comparison alignment", () => {
  it("runBacktest(assembled) deep-equals runBacktest(loadFixture(export))", () => {
    const {
      pricesByArea,
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    } = buildDbShaped();
    const assembled = assembleBacktestData(
      pricesByArea,
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    );
    const summaryA = runBacktest(assembled);

    const tmp = mkdtempSync(path.join(tmpdir(), "bt-roundtrip-"));
    try {
      const file = path.join(tmp, "snapshot.json");
      writeFileSync(file, toFixtureJson(assembled), "utf-8");
      const summaryB = runBacktest(loadFixture(file));
      expect(summaryB.origins).toBe(summaryA.origins);
      expect(summaryB.preVintageOrigins).toBe(summaryA.preVintageOrigins);
      expect(summaryB.leakFree).toBe(summaryA.leakFree);
      expect(summaryB.modelMae).toBe(summaryA.modelMae);
      expect(summaryB.residuals).toEqual(summaryA.residuals);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("--compare scores an identical origin set for both runs", () => {
    const {
      pricesByArea,
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    } = buildDbShaped();
    const assembled = assembleBacktestData(
      pricesByArea,
      fingridActualsByDataset,
      fingridForecastVintagesByDataset,
    );
    const cmp = compareOptimism(assembled);
    expect(cmp.scoredOrigins).toBeGreaterThan(0);
    expect(cmp.leaked.scoredOriginsMs.size).toBe(cmp.scoredOrigins);
    expect(cmp.honest.scoredOriginsMs.size).toBe(cmp.scoredOrigins);
    // Same origins scored on both sides.
    for (const ms of cmp.honest.scoredOriginsMs) {
      expect(cmp.leaked.scoredOriginsMs.has(ms)).toBe(true);
    }
  });
});

describe("old-shape fixture degrade (never fabricate issuedAt)", () => {
  it("loads an old single-`fingrid` fixture with actuals only, forecasts empty", () => {
    const start = Date.parse("2026-02-01T00:00:00.000Z");
    const wind: { startTime: string; endTime: string; value: number }[] = [];
    const cons: { startTime: string; endTime: string; value: number }[] = [];
    const prices: { start: string; spotCentsKwh: number }[] = [];
    for (let q = 0; q < 10 * 96; q++) {
      const ms = start + q * QUARTER_MS;
      prices.push({ start: new Date(ms).toISOString(), spotCentsKwh: 2 });
      const rec = {
        startTime: new Date(ms).toISOString(),
        endTime: new Date(ms + QUARTER_MS).toISOString(),
        value: 3000,
      };
      wind.push(rec);
      cons.push(rec);
    }
    const oldShape = {
      prices,
      fingrid: { "245": wind, "75": wind, "165": cons, "124": cons },
    };
    const tmp = mkdtempSync(path.join(tmpdir(), "bt-oldshape-"));
    try {
      const file = path.join(tmp, "old.json");
      writeFileSync(file, JSON.stringify(oldShape), "utf-8");
      const data = loadFixture(file);
      // Actuals map through; forecast vintages are EMPTY (no issuedAt to invent).
      expect(Object.keys(data.fingridActualsByDataset).sort()).toEqual([
        "124",
        "75",
      ]);
      expect(Object.keys(data.fingridForecastVintagesByDataset)).toHaveLength(
        0,
      );
      // With no vintages, every origin is pre-vintage → nothing is scored.
      const summary = runBacktest(data);
      expect(summary.origins).toBe(0);
      expect(summary.preVintageOrigins).toBeGreaterThan(0);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});

describe("parseWindowDays", () => {
  it("defaults to DEFAULT_WINDOW_DAYS when absent", () => {
    expect(parseWindowDays(undefined)).toBe(DEFAULT_WINDOW_DAYS);
  });

  it("accepts a positive integer", () => {
    expect(parseWindowDays("120")).toBe(120);
  });

  it("rejects non-integers, zero, negatives, and junk", () => {
    expect(() => parseWindowDays("0")).toThrow();
    expect(() => parseWindowDays("-5")).toThrow();
    expect(() => parseWindowDays("30.5")).toThrow();
    expect(() => parseWindowDays("abc")).toThrow();
    expect(() => parseWindowDays("30days")).toThrow();
  });
});

describe("thin-data guard", () => {
  it("flags fewer than MIN_SCOREABLE_ORIGINS as thin", () => {
    expect(isThinData(MIN_SCOREABLE_ORIGINS - 1)).toBe(true);
    expect(isThinData(0)).toBe(true);
  });

  it("treats at/above the threshold as a verdict", () => {
    expect(isThinData(MIN_SCOREABLE_ORIGINS)).toBe(false);
    expect(isThinData(MIN_SCOREABLE_ORIGINS + 50)).toBe(false);
  });
});

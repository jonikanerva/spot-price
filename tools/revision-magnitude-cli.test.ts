import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  assembleStudyInput,
  loadVintageFixture,
  toFixtureJson,
} from "./revision-magnitude-cli.js";
import { runRevisionStudy, type ActualRecord } from "./revision-magnitude.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";
import type { ForecastVintageRecord } from "../src/types.js";

const HOUR = 60 * 60 * 1000;
const iso = (ms: number): string => new Date(ms).toISOString();

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

/** Build store-shaped vintages (both datasets) plus paired actuals. */
const buildStoreShaped = (): {
  vintagesByDataset: Record<string, ForecastVintageRecord[]>;
  actualsByDataset: Record<string, ActualRecord[]>;
} => {
  const base = Date.parse("2026-06-25T00:00:00.000Z");
  const wind: ForecastVintageRecord[] = [];
  const cons: ForecastVintageRecord[] = [];
  const windActual: ActualRecord[] = [];
  const consActual: ActualRecord[] = [];
  for (let t = 0; t < 40; t++) {
    const target = base + t * 6 * HOUR;
    const w = 2000 + ((t * 131) % 1500);
    const c = 8000 + (t % 24) * 120;
    wind.push(vintage(DATASET_WIND_FORECAST, target, -1, w));
    wind.push(vintage(DATASET_WIND_FORECAST, target, 12, w + 200));
    wind.push(vintage(DATASET_WIND_FORECAST, target, 24, w + 350));
    cons.push(vintage(DATASET_CONSUMPTION_FORECAST, target, -1, c));
    cons.push(vintage(DATASET_CONSUMPTION_FORECAST, target, 12, c + 40));
    windActual.push({
      datasetId: DATASET_WIND_ACTUAL,
      startTime: iso(target),
      value: w - 5,
    });
    consActual.push({
      datasetId: DATASET_CONSUMPTION_ACTUAL,
      startTime: iso(target),
      value: c + 3,
    });
  }
  return {
    vintagesByDataset: {
      [String(DATASET_WIND_FORECAST)]: wind,
      [String(DATASET_CONSUMPTION_FORECAST)]: cons,
    },
    actualsByDataset: {
      [String(DATASET_WIND_ACTUAL)]: windActual,
      [String(DATASET_CONSUMPTION_ACTUAL)]: consActual,
    },
  };
};

describe("fixture round-trip — parity with loadVintageFixture", () => {
  it("assembled input deep-equals the export → load round-trip (incl. endTime)", () => {
    const { vintagesByDataset, actualsByDataset } = buildStoreShaped();
    const assembled = assembleStudyInput(vintagesByDataset, actualsByDataset);

    const tmp = mkdtempSync(path.join(tmpdir(), "rm-parity-"));
    try {
      const file = path.join(tmp, "vintages.json");
      writeFileSync(file, toFixtureJson(assembled), "utf-8");
      const loaded = loadVintageFixture(file);
      expect(loaded).toEqual(assembled);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("runRevisionStudy(assembled) deep-equals runRevisionStudy(loaded)", () => {
    const { vintagesByDataset, actualsByDataset } = buildStoreShaped();
    const assembled = assembleStudyInput(vintagesByDataset, actualsByDataset);
    const summaryA = runRevisionStudy(assembled);

    const tmp = mkdtempSync(path.join(tmpdir(), "rm-roundtrip-"));
    try {
      const file = path.join(tmp, "snapshot.json");
      writeFileSync(file, toFixtureJson(assembled), "utf-8");
      const summaryB = runRevisionStudy(loadVintageFixture(file));
      // Catches Date-vs-ISO drift, string-vs-number keys, and dropped fields.
      expect(summaryB).toEqual(summaryA);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  it("a fixture with no actuals still loads and runs", () => {
    const { vintagesByDataset } = buildStoreShaped();
    const assembled = assembleStudyInput(vintagesByDataset, {});
    const tmp = mkdtempSync(path.join(tmpdir(), "rm-noact-"));
    try {
      const file = path.join(tmp, "v.json");
      writeFileSync(file, toFixtureJson(assembled), "utf-8");
      const loaded = loadVintageFixture(file);
      expect(loaded).toEqual(assembled);
      const summary = runRevisionStudy(loaded);
      const w = summary.datasets.find(
        (d) => d.datasetId === DATASET_WIND_FORECAST,
      );
      expect(w?.actualCheck).toBeNull();
      expect(w?.admissibleTargets).toBeGreaterThan(0);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  assembleStudyInput,
  loadVintageFixture,
  rowToVintage,
  toFixtureJson,
  type VintageRow,
} from "./vintage-revision-cli.js";
import { runRevisionStudy, type ActualRecord } from "./vintage-revision.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";

const HOUR = 60 * 60 * 1000;
const iso = (ms: number): string => new Date(ms).toISOString();

/** A raw `fingrid_forecasts` row (snake_case), as the DB driver returns it. */
const row = (
  datasetId: number,
  targetMs: number,
  leadH: number,
  value: number,
): VintageRow => ({
  dataset_id: datasetId,
  issued_at: iso(targetMs - leadH * HOUR),
  start_time: iso(targetMs),
  end_time: iso(targetMs + 15 * 60 * 1000),
  value,
});

/** Build DB-shaped rows for both forecast datasets plus paired actuals. */
const buildDbShaped = (): {
  vintagesByDataset: Record<string, ReturnType<typeof rowToVintage>[]>;
  actualsByDataset: Record<string, ActualRecord[]>;
} => {
  const base = Date.parse("2026-06-25T00:00:00.000Z");
  const wind: VintageRow[] = [];
  const cons: VintageRow[] = [];
  const windActual: ActualRecord[] = [];
  const consActual: ActualRecord[] = [];
  for (let t = 0; t < 40; t++) {
    const target = base + t * 6 * HOUR;
    const w = 2000 + ((t * 131) % 1500);
    const c = 8000 + (t % 24) * 120;
    wind.push(row(DATASET_WIND_FORECAST, target, -1, w));
    wind.push(row(DATASET_WIND_FORECAST, target, 12, w + 200));
    wind.push(row(DATASET_WIND_FORECAST, target, 24, w + 350));
    cons.push(row(DATASET_CONSUMPTION_FORECAST, target, -1, c));
    cons.push(row(DATASET_CONSUMPTION_FORECAST, target, 12, c + 40));
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
      [String(DATASET_WIND_FORECAST)]: wind.map(rowToVintage),
      [String(DATASET_CONSUMPTION_FORECAST)]: cons.map(rowToVintage),
    },
    actualsByDataset: {
      [String(DATASET_WIND_ACTUAL)]: windActual,
      [String(DATASET_CONSUMPTION_ACTUAL)]: consActual,
    },
  };
};

describe("rowToVintage", () => {
  it("maps snake_case DB columns to the engine's VintageRecord, dropping end_time", () => {
    const r = row(
      DATASET_WIND_FORECAST,
      Date.parse("2026-06-25T00:00:00.000Z"),
      12,
      2500,
    );
    expect(rowToVintage(r)).toEqual({
      datasetId: DATASET_WIND_FORECAST,
      issuedAt: r.issued_at,
      startTime: r.start_time,
      value: 2500,
    });
  });
});

describe("fixture round-trip — parity with loadVintageFixture", () => {
  it("assembled input deep-equals the export → load round-trip", () => {
    const { vintagesByDataset, actualsByDataset } = buildDbShaped();
    const assembled = assembleStudyInput(vintagesByDataset, actualsByDataset);

    const tmp = mkdtempSync(path.join(tmpdir(), "vr-parity-"));
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
    const { vintagesByDataset, actualsByDataset } = buildDbShaped();
    const assembled = assembleStudyInput(vintagesByDataset, actualsByDataset);
    const summaryA = runRevisionStudy(assembled);

    const tmp = mkdtempSync(path.join(tmpdir(), "vr-roundtrip-"));
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
    const { vintagesByDataset } = buildDbShaped();
    const assembled = assembleStudyInput(vintagesByDataset, {});
    const tmp = mkdtempSync(path.join(tmpdir(), "vr-noact-"));
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

/**
 * Writes the golden fixtures of the Rust forecast port to
 * `crates/domain/tests/golden/forecast/`. Run with
 * `pnpm tsx scripts/golden/forecast.ts`.
 *
 * The script imports the real TypeScript forecast modules. It builds a
 * deterministic data set from `tools/backtest-data/fixture.json` (one real
 * day, tiled with a seeded generator), writes the inputs, and writes the
 * outputs of the TypeScript functions for these inputs. The Rust tests read
 * the inputs from the fixtures; they never run a generator.
 *
 * `toForecastEntries` is private in `src/routes/forecast.ts`. The function
 * `entriesFor` below repeats its short mapping and calls the real
 * `applyContractTerms`, `extractHourInTimeZone`, and
 * `formatDateTimeInTimeZone`.
 */
import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { format } from "prettier";
import {
  alignSeries,
  applyWithinDayShape,
  bucketRecords,
  buildForecast,
  buildPredictedSeries,
  expandHourlyToQuarters,
  extendWithLastWeek,
  fitLinear,
  quarterKey,
  sanityBoundFromHistory,
  type ForecastOptions,
} from "../../src/forecast.js";
import {
  buildFeatureVector,
  buildFeatureVectorIndexed,
  buildForecastPriceIndex,
  buildTrainingMatrix,
  FEATURE_NAMES,
  type FeatureContext,
} from "../../src/features.js";
import { createRidgeModel, type FitOptions } from "../../src/model.js";
import {
  applyBand,
  buildArtifact,
  deriveBandOffsets,
  observedCoverageOf,
  type BandOffsets,
  type CalibratedBands,
  type HorizonResidual,
} from "../../src/conformal.js";
import {
  applyContractTerms,
  extractHourInTimeZone,
} from "../../src/calculator.js";
import { formatDateTimeInTimeZone } from "../../src/time.js";
import type {
  FingridRecord,
  ForecastResult,
  UserSettings,
} from "../../src/types.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, "..", "..");
const outDir = path.join(
  root,
  "crates",
  "domain",
  "tests",
  "golden",
  "forecast",
);

// ---------------------------------------------------------------------------
// Deterministic generator
// ---------------------------------------------------------------------------

/** mulberry32: a small seeded PRNG with uniform output in [0, 1). */
const prng = (seed: number): (() => number) => {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

const round = (value: number, digits: number): number => {
  const f = 10 ** digits;
  return Math.round(value * f) / f;
};

interface FixtureFile {
  readonly prices: readonly { start: string; spotCentsKwh: number }[];
  readonly fingridActuals: Readonly<
    Record<string, readonly { startTime: string; value: number }[]>
  >;
}

const fixture = JSON.parse(
  readFileSync(
    path.join(root, "tools", "backtest-data", "fixture.json"),
    "utf8",
  ),
) as FixtureFile;

const dayProfile = (values: readonly number[]): number[] => {
  if (values.length !== 96) {
    throw new Error(`expected 96 quarters, got ${String(values.length)}`);
  }
  return [...values];
};

const priceDay = dayProfile(fixture.prices.map((p) => p.spotCentsKwh));
const windDay = dayProfile(
  (fixture.fingridActuals["75"] ?? []).map((r) => r.value),
);
const consDay = dayProfile(
  (fixture.fingridActuals["124"] ?? []).map((r) => r.value),
);

/** A regular series: values from `startMs` every `stepMs`, without `gaps`. */
interface Run {
  readonly startMs: number;
  readonly stepMs: number;
  readonly values: number[];
  readonly gaps: number[];
}

/** Records in columns: `startMs[i]` and `value[i]`. */
interface RecordColumns {
  readonly startMs: number[];
  readonly value: number[];
}

const BASE_START_MS = Date.parse("2026-02-20T00:00:00.000Z");
const BASE_DAYS = 48;
const BASE_QUARTERS = BASE_DAYS * 96;

const buildDataset = (): {
  fi: Run;
  se1: Run;
  se3: Run;
  ee: Run;
  windActual: RecordColumns;
  consActual: RecordColumns;
  windForecast: RecordColumns;
  consForecast: RecordColumns;
} => {
  const rand = prng(20260302);
  const dayLevel: number[] = [];
  const dayWind: number[] = [];
  for (let d = 0; d < BASE_DAYS; d++) {
    dayLevel.push(rand());
    dayWind.push(rand());
  }
  const fiValues: number[] = [];
  const se1Values: number[] = [];
  const se3Values: number[] = [];
  const eeValues: number[] = [];
  for (let i = 0; i < BASE_QUARTERS; i++) {
    const d = Math.floor(i / 96);
    const q = i % 96;
    const level = dayLevel[d] ?? 0;
    const windy = (dayWind[d] ?? 0) > 0.85;
    const base =
      (priceDay[q] ?? 0) * (0.6 + 0.8 * level) +
      2.5 * Math.sin((2 * Math.PI * d) / 9) +
      (rand() - 0.5) * 0.8 -
      (windy ? 7 : 0);
    fiValues.push(round(base, 3));
    se1Values.push(round(base * 0.7 + (rand() - 0.5), 3));
    se3Values.push(round(base * 0.9 + (rand() - 0.5), 3));
    eeValues.push(round(base * 1.1 + (rand() - 0.5) * 2, 3));
  }
  // A partial gap on day 10 and a missing whole day 22 in the FI prices.
  const fiGaps: number[] = [];
  for (let q = 40; q < 48; q++) fiGaps.push(10 * 96 + q);
  for (let q = 0; q < 96; q++) fiGaps.push(22 * 96 + q);
  // EE is missing on days 30–34.
  const eeGaps: number[] = [];
  for (let i = 30 * 96; i < 35 * 96; i++) eeGaps.push(i);

  const windActual: RecordColumns = { startMs: [], value: [] };
  const consActual: RecordColumns = { startMs: [], value: [] };
  const windForecast: RecordColumns = { startMs: [], value: [] };
  const consForecast: RecordColumns = { startMs: [], value: [] };
  for (let i = 0; i < BASE_QUARTERS; i++) {
    const d = Math.floor(i / 96);
    const q = i % 96;
    const ms = BASE_START_MS + i * QUARTER_MS;
    const wind = round(
      (windDay[q] ?? 0) * (0.3 + 1.4 * (dayWind[d] ?? 0)) +
        (rand() - 0.5) * 200,
      1,
    );
    const cons = round(
      (consDay[q] ?? 0) * (0.9 + 0.1 * Math.cos((2 * Math.PI * d) / 7)) +
        (rand() - 0.5) * 150,
      1,
    );
    windActual.startMs.push(ms);
    windActual.value.push(wind);
    // Day 5 has a second wind sample in each quarter, to test bucketing means.
    if (d === 5) {
      windActual.startMs.push(ms + 5 * 60 * 1000);
      windActual.value.push(round(wind + 37.5, 1));
    }
    // Consumption actuals are hourly before day 15, quarterly after.
    if (d >= 15 || q % 4 === 0) {
      consActual.startMs.push(ms);
      consActual.value.push(cons);
    }
    windForecast.startMs.push(ms);
    windForecast.value.push(round(wind + (rand() - 0.5) * 400, 1));
    consForecast.startMs.push(ms);
    consForecast.value.push(round(cons + (rand() - 0.5) * 300, 1));
  }
  const run = (values: number[], gaps: number[]): Run => ({
    startMs: BASE_START_MS,
    stepMs: QUARTER_MS,
    values,
    gaps,
  });
  return {
    fi: run(fiValues, fiGaps),
    se1: run(se1Values, []),
    se3: run(se3Values, []),
    ee: run(eeValues, eeGaps),
    windActual,
    consActual,
    windForecast,
    consForecast,
  };
};

const dataset = buildDataset();

// ---------------------------------------------------------------------------
// Scenario inputs
// ---------------------------------------------------------------------------

type Range = readonly [number, number] | null;

interface ScenarioRanges {
  readonly fi: Range;
  readonly se1: Range;
  readonly se3: Range;
  readonly ee: Range;
  readonly windActual: Range;
  readonly consActual: Range;
  readonly windForecast: Range;
  readonly consForecast: Range;
}

const runMap = (run: Run, range: Range): Map<string, number> => {
  const out = new Map<string, number>();
  if (range === null) {
    return out;
  }
  const gaps = new Set(run.gaps);
  run.values.forEach((value, i) => {
    const ms = run.startMs + i * run.stepMs;
    if (!gaps.has(i) && ms >= range[0] && ms < range[1]) {
      out.set(quarterKey(ms), value);
    }
  });
  return out;
};

const records = (cols: RecordColumns, range: Range): FingridRecord[] => {
  const out: FingridRecord[] = [];
  if (range === null) {
    return out;
  }
  cols.startMs.forEach((ms, i) => {
    if (ms >= range[0] && ms < range[1]) {
      out.push({
        datasetId: 0,
        startTime: new Date(ms).toISOString(),
        endTime: new Date(ms + QUARTER_MS).toISOString(),
        value: cols.value[i] ?? 0,
      });
    }
  });
  return out;
};

const STUB_BANDS: CalibratedBands = {
  method: "empirical-residual",
  nominalCoverage: 0.8,
  observedCoverage: 0.83,
  calibrated: true,
  offsetsByHour: new Map<number, BandOffsets>([
    [0, { lowOffset: -0.31, highOffset: 0.27 }],
    [7, { lowOffset: -0.52, highOffset: 0.61 }],
    [12, { lowOffset: -0.2, highOffset: 0.9 }],
    [18, { lowOffset: 0, highOffset: 0 }],
  ]),
  globalOffsets: { lowOffset: -0.4, highOffset: 0.35 },
  generatedAt: "2026-06-01T00:00:00.000Z",
};

interface ScenarioOptions {
  readonly sanityBound?: { readonly min: number; readonly max: number };
  readonly ridgeLambda?: number;
  readonly windExtensionWeeks?: number;
  readonly consumptionExtensionWeeks?: number;
  readonly stubBands?: boolean;
}

interface SettingsCase {
  readonly name: string;
  readonly timeZone: string;
  readonly settings: UserSettings;
}

const HELSINKI: SettingsCase = {
  name: "helsinki",
  timeZone: "Europe/Helsinki",
  settings: {
    userId: "golden",
    marginCentsKwh: 0.49,
    transferDayCentsKwh: 3.18,
    transferNightCentsKwh: 1.85,
    taxCentsKwh: 2.827515,
    vatPercent: 25.5,
    nightStartHour: 22,
    nightEndHour: 7,
    timezone: "Europe/Helsinki",
    area: "FI",
  },
};
const KATHMANDU: SettingsCase = {
  name: "kathmandu",
  timeZone: "Asia/Kathmandu",
  settings: {
    ...HELSINKI.settings,
    nightStartHour: 1,
    nightEndHour: 6,
    vatPercent: 24,
    timezone: "Asia/Kathmandu",
  },
};
const ST_JOHNS: SettingsCase = {
  name: "st_johns",
  timeZone: "America/St_Johns",
  settings: { ...HELSINKI.settings, timezone: "America/St_Johns" },
};

interface Scenario {
  readonly name: string;
  readonly seriesStartMs: number;
  readonly seriesEndMs: number;
  readonly ranges: ScenarioRanges;
  readonly options: ScenarioOptions;
  /** True: run the route sequence (grid check + history sanity bound). */
  readonly route: boolean;
  readonly entries: readonly SettingsCase[];
}

const at = (iso: string): number => Date.parse(iso);

/** Ranges of a normal issue at `startMs`: forecasts reach `horizonHours`. */
const normalRanges = (
  startMs: number,
  horizonHours: number,
): ScenarioRanges => {
  const historyStart = startMs - 30 * DAY_MS;
  return {
    fi: [historyStart, startMs],
    se1: [historyStart, startMs],
    se3: [historyStart, startMs],
    ee: [historyStart, startMs],
    windActual: [historyStart, startMs - 2 * HOUR_MS],
    consActual: [historyStart, startMs - 2 * HOUR_MS],
    windForecast: [historyStart, startMs + horizonHours * HOUR_MS],
    consForecast: [historyStart, startMs + horizonHours * HOUR_MS],
  };
};

const scenario = (
  name: string,
  startIso: string,
  build: (startMs: number) => Partial<Omit<Scenario, "name">>,
): Scenario => {
  const startMs = at(startIso);
  return {
    name,
    seriesStartMs: startMs,
    seriesEndMs: startMs + 3 * DAY_MS,
    ranges: normalRanges(startMs, 36),
    options: {},
    route: true,
    entries: [],
    ...build(startMs),
  };
};

const scenarios: Scenario[] = [
  scenario("normal_midnight", "2026-03-25T00:00:00.000Z", () => ({
    entries: [HELSINKI],
  })),
  scenario("dst_week", "2026-03-28T00:00:00.000Z", () => ({
    entries: [HELSINKI, KATHMANDU, ST_JOHNS],
  })),
  scenario("midday_issue", "2026-03-30T13:15:00.000Z", () => ({
    entries: [HELSINKI],
  })),
  scenario("earlier_issue", "2026-03-22T00:00:00.000Z", () => ({})),
  scenario("grid_outage", "2026-03-25T00:00:00.000Z", (s) => ({
    ranges: {
      ...normalRanges(s, 36),
      windActual: null,
      consActual: null,
      windForecast: null,
      consForecast: null,
    },
  })),
  scenario("grid_outage_pipeline", "2026-03-25T00:00:00.000Z", (s) => ({
    route: false,
    ranges: {
      ...normalRanges(s, 36),
      windActual: null,
      consActual: null,
      windForecast: null,
      consForecast: null,
    },
  })),
  scenario("future_grid_outage", "2026-03-26T00:00:00.000Z", (s) => ({
    ranges: {
      ...normalRanges(s, 36),
      windForecast: [s - 30 * DAY_MS, s],
      consForecast: [s - 30 * DAY_MS, s],
    },
  })),
  scenario("no_forecast_records", "2026-03-26T00:00:00.000Z", (s) => ({
    ranges: { ...normalRanges(s, 36), windForecast: null, consForecast: null },
  })),
  scenario("no_neighbours", "2026-03-25T00:00:00.000Z", (s) => ({
    ranges: { ...normalRanges(s, 36), se1: null, se3: null, ee: null },
  })),
  scenario("partial_neighbours", "2026-03-27T00:00:00.000Z", (s) => ({
    ranges: { ...normalRanges(s, 36), se1: null },
  })),
  scenario("tight_clamp", "2026-03-25T00:00:00.000Z", () => ({
    route: false,
    options: { sanityBound: { min: 2, max: 6 } },
  })),
  scenario("short_history", "2026-03-25T00:00:00.000Z", (s) => ({
    ranges: { ...normalRanges(s, 36), fi: [s - 20 * QUARTER_MS, s] },
    entries: [HELSINKI],
  })),
  scenario("no_prices", "2026-03-25T00:00:00.000Z", (s) => ({
    ranges: {
      ...normalRanges(s, 36),
      fi: null,
      se1: null,
      se3: null,
      ee: null,
    },
  })),
  scenario("bands_and_options", "2026-03-29T00:00:00.000Z", () => ({
    route: false,
    options: {
      stubBands: true,
      ridgeLambda: 0.25,
      windExtensionWeeks: 2,
      consumptionExtensionWeeks: 3,
    },
    entries: [HELSINKI, KATHMANDU],
  })),
  scenario("unaligned_bounds", "2026-03-24T06:07:00.000Z", (s) => ({
    seriesEndMs: s + 2 * DAY_MS + 7 * 60 * 1000,
    route: false,
  })),
];

// ---------------------------------------------------------------------------
// Scenario outputs
// ---------------------------------------------------------------------------

/** The response mapping of `toForecastEntries` in `src/routes/forecast.ts`. */
const entriesFor = (result: ForecastResult, c: SettingsCase): unknown => {
  const cols = {
    name: c.name,
    timeZone: c.timeZone,
    settings: c.settings,
    start: [] as string[],
    end: [] as string[],
    localStart: [] as string[],
    localEnd: [] as string[],
    spot: [] as number[],
    total: [] as number[],
    spotLow: [] as number[],
    spotHigh: [] as number[],
    totalLow: [] as number[],
    totalHigh: [] as number[],
  };
  for (const point of result.series) {
    const startMs = new Date(point.start).getTime();
    const endIso = new Date(startMs + QUARTER_MS).toISOString();
    const hour = extractHourInTimeZone(point.start, c.settings.timezone);
    const breakdown = applyContractTerms(
      point.estimatedSpotCentsKwh,
      c.settings,
      hour,
    );
    cols.start.push(point.start);
    cols.end.push(endIso);
    cols.localStart.push(
      formatDateTimeInTimeZone(point.start, c.settings.timezone),
    );
    cols.localEnd.push(formatDateTimeInTimeZone(endIso, c.settings.timezone));
    cols.spot.push(breakdown.spotCentsKwh);
    cols.total.push(breakdown.totalCentsKwh);
    const low = point.estimatedSpotLowCentsKwh;
    const high = point.estimatedSpotHighCentsKwh;
    if (low !== undefined && high !== undefined) {
      cols.spotLow.push(low);
      cols.spotHigh.push(high);
      cols.totalLow.push(
        applyContractTerms(low, c.settings, hour).totalCentsKwh,
      );
      cols.totalHigh.push(
        applyContractTerms(high, c.settings, hour).totalCentsKwh,
      );
    }
  }
  return cols;
};

const SAMPLE_ROWS = 4;

const runScenario = (s: Scenario): unknown => {
  const spot = runMap(dataset.fi, s.ranges.fi);
  const neighbours = new Map<string, Map<string, number>>();
  for (const [area, run, range] of [
    ["SE1", dataset.se1, s.ranges.se1],
    ["SE3", dataset.se3, s.ranges.se3],
    ["EE", dataset.ee, s.ranges.ee],
  ] as const) {
    if (range !== null) {
      neighbours.set(area, runMap(run, range));
    }
  }
  const windForecast = records(dataset.windForecast, s.ranges.windForecast);
  const windActual = records(dataset.windActual, s.ranges.windActual);
  const consForecast = records(dataset.consForecast, s.ranges.consForecast);
  const consActual = records(dataset.consActual, s.ranges.consActual);

  const opts: ForecastOptions = {
    ...(s.options.sanityBound !== undefined
      ? { sanityBound: s.options.sanityBound }
      : {}),
    ...(s.options.ridgeLambda !== undefined
      ? { ridgeLambda: s.options.ridgeLambda }
      : {}),
    ...(s.options.windExtensionWeeks !== undefined
      ? { windExtensionWeeks: s.options.windExtensionWeeks }
      : {}),
    ...(s.options.consumptionExtensionWeeks !== undefined
      ? { consumptionExtensionWeeks: s.options.consumptionExtensionWeeks }
      : {}),
    ...(s.options.stubBands === true ? { bands: STUB_BANDS } : {}),
  };
  const routeSanity = sanityBoundFromHistory(spot);
  const effective: ForecastOptions =
    s.route && routeSanity !== null
      ? { ...opts, sanityBound: routeSanity }
      : opts;
  const gridCount =
    windForecast.length +
    windActual.length +
    consForecast.length +
    consActual.length;

  const input = {
    spotPricesByKey: spot,
    neighborPricesByArea: neighbours,
    windForecast,
    windActual,
    consumptionForecast: consForecast,
    consumptionActual: consActual,
    seriesStartMs: s.seriesStartMs,
    seriesEndMs: s.seriesEndMs,
  };
  const result = buildForecast(input, effective);

  // The intermediates: the same steps as `buildForecast`, with the real
  // exported functions, before rounding.
  const startMs = Math.floor(s.seriesStartMs / QUARTER_MS) * QUARTER_MS;
  const endMs = Math.floor(s.seriesEndMs / QUARTER_MS) * QUARTER_MS;
  const windQ = bucketRecords(windForecast);
  const windActualQ = bucketRecords(windActual);
  const consQ = bucketRecords(consForecast);
  const consActualQ = expandHourlyToQuarters(consActual);
  const consExtended = extendWithLastWeek(
    consQ,
    consActualQ,
    endMs,
    effective.consumptionExtensionWeeks ?? 1,
  );
  const windExtended = extendWithLastWeek(
    windQ,
    windActualQ,
    endMs,
    effective.windExtensionWeeks ?? 4,
  );
  const windByKey = new Map<string, number>(windActualQ);
  for (const [k, v] of windExtended) windByKey.set(k, v);
  const consumptionByKey = new Map<string, number>(consActualQ);
  for (const [k, v] of consExtended) consumptionByKey.set(k, v);
  const ctx: FeatureContext = {
    fiPricesByKey: spot,
    neighborPricesByArea: neighbours,
    windByKey,
    consumptionByKey,
  };
  const index = buildForecastPriceIndex(ctx);
  const training = buildTrainingMatrix(startMs, ctx, index);
  const fitted = createRidgeModel().fit(
    { features: training.features, targets: training.targets },
    effective.ridgeLambda !== undefined
      ? { ridgeLambda: effective.ridgeLambda }
      : {},
  );
  const numQuarters = Math.max(0, Math.trunc((endMs - startMs) / QUARTER_MS));
  const raw = new Map<string, number>();
  for (let i = 0; i < numQuarters; i++) {
    const ms = startMs + i * QUARTER_MS;
    raw.set(
      quarterKey(ms),
      fitted.predict(buildFeatureVectorIndexed(ms, ctx, index)),
    );
  }
  const shaped = applyWithinDayShape(raw, spot);
  const bound = effective.sanityBound;
  const clamped = [...shaped.result.values()].map((v) =>
    bound === undefined ? v : Math.min(bound.max, Math.max(bound.min, v)),
  );
  const built = buildPredictedSeries(
    new Map([...shaped.result.keys()].map((k, i) => [k, clamped[i] ?? 0])),
    startMs,
    numQuarters,
  );
  built.series.forEach((p, i) => {
    if (p.estimatedSpotCentsKwh !== result.series[i]?.estimatedSpotCentsKwh) {
      throw new Error(
        `${s.name}: the intermediates do not reproduce buildForecast`,
      );
    }
  });

  const n = training.targets.length;
  const sampleIndices =
    n === 0
      ? []
      : [...new Set([0, 1, Math.floor(n / 2), n - 1].filter((i) => i < n))];
  const banded = result.series.filter(
    (p) => p.estimatedSpotLowCentsKwh !== undefined,
  );

  return {
    name: s.name,
    seriesStartMs: s.seriesStartMs,
    seriesEndMs: s.seriesEndMs,
    ranges: s.ranges,
    options: s.options,
    route: s.route,
    expected: {
      gridRecordCount: gridCount,
      routeSanityBound: routeSanity,
      training: {
        count: n,
        sampleIndices,
        sampleKeysMs: sampleIndices.map((i) =>
          Date.parse(training.keys[i] ?? ""),
        ),
        sampleRows: sampleIndices.map((i) => training.features.rows[i] ?? []),
        sampleTargets: sampleIndices.map((i) => training.targets[i] ?? 0),
      },
      neutral: index.neutral,
      rawPredictions: [...raw.values()],
      shaped: [...shaped.result.values()],
      clamped,
      series: {
        startMs: result.series.map((p) => Date.parse(p.start)),
        estimate: result.series.map((p) => p.estimatedSpotCentsKwh),
        bandedCount: banded.length,
        low: banded.map((p) => p.estimatedSpotLowCentsKwh ?? 0),
        high: banded.map((p) => p.estimatedSpotHighCentsKwh ?? 0),
      },
      diagnostics: result.diagnostics,
      assessment: {
        degraded:
          result.diagnostics.fitUsedDefault ||
          result.diagnostics.zeroSeededQuarters > 0,
        reason: result.diagnostics.fitUsedDefault
          ? "Insufficient price history to fit the model; using a default estimate"
          : result.diagnostics.zeroSeededQuarters > 0
            ? "Some quarters had no input data and were zero-seeded"
            : null,
      },
      entries: s.entries.map((c) => entriesFor(result, c)),
    },
  };
};

// ---------------------------------------------------------------------------
// Unit vectors
// ---------------------------------------------------------------------------

const ANCHOR = at("2026-03-01T00:00:00.000Z");

const mapEntries = (
  m: ReadonlyMap<string, number>,
): { keyMs: number[]; value: number[] } => {
  const sorted = [...m.entries()].sort((a, b) =>
    a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0,
  );
  return {
    keyMs: sorted.map(([k]) => Date.parse(k)),
    value: sorted.map(([, v]) => v),
  };
};

const toMap = (
  keyMs: readonly number[],
  value: readonly number[],
): Map<string, number> =>
  new Map(keyMs.map((ms, i) => [quarterKey(ms), value[i] ?? 0]));

const rec = (startMs: number, value: number): FingridRecord => ({
  datasetId: 0,
  startTime: new Date(startMs).toISOString(),
  endTime: new Date(startMs + QUARTER_MS).toISOString(),
  value,
});

const seriesUnits = (): unknown => {
  const rand = prng(77);
  const quarterKeys = [
    "2026-03-01T10:07:33.500Z",
    "2026-03-01T10:59:59.999Z",
    "2026-03-01T00:00:00.000Z",
    "1969-12-31T23:59:59.999Z",
    "2026-10-25T01:44:00.000Z",
  ].map((iso) => ({ ms: Date.parse(iso), key: quarterKey(Date.parse(iso)) }));

  const bucketInput: RecordColumns = { startMs: [], value: [] };
  for (let i = 0; i < 40; i++) {
    bucketInput.startMs.push(ANCHOR + Math.floor(rand() * 12) * 5 * 60 * 1000);
    bucketInput.value.push(round(rand() * 1000, 2));
  }
  const bucketRecs = records(bucketInput, [-Infinity, Infinity]);

  const hourlyInput: RecordColumns = {
    startMs: [
      ANCHOR,
      ANCHOR + HOUR_MS,
      ANCHOR + HOUR_MS + QUARTER_MS,
      ANCHOR + 3 * HOUR_MS + 2 * QUARTER_MS,
      ANCHOR + 5 * HOUR_MS,
      ANCHOR + 5 * HOUR_MS + 60_000,
    ],
    value: [100, 200, 250, 300, 400, 500],
  };

  const extendCases = [1, 2, 4].map((weeks) => {
    const forecast = new Map<string, number>();
    for (let q = 0; q < 10; q++)
      forecast.set(quarterKey(ANCHOR + q * QUARTER_MS), 1000 + q);
    const actual = new Map<string, number>();
    for (let q = -5 * 7 * 96; q < 0; q++) {
      if (rand() < 0.8)
        actual.set(
          quarterKey(ANCHOR + q * QUARTER_MS),
          round(rand() * 3000, 1),
        );
    }
    const end = ANCHOR + 2 * DAY_MS + 3 * QUARTER_MS;
    const out = extendWithLastWeek(forecast, actual, end, weeks);
    return {
      weeks,
      seriesEndMs: end,
      forecast: mapEntries(forecast),
      actual: mapEntries(actual),
      expected: mapEntries(out),
    };
  });

  const alignPrice = new Map<string, number>();
  const alignResidual = new Map<string, number>();
  for (let q = 0; q < 30; q++) {
    if (rand() < 0.8)
      alignPrice.set(
        quarterKey(ANCHOR + q * QUARTER_MS),
        round(rand() * 10, 3),
      );
    if (rand() < 0.8)
      alignResidual.set(
        quarterKey(ANCHOR + q * QUARTER_MS),
        round(rand() * 5000, 1),
      );
  }
  const aligned = alignSeries(alignPrice, alignResidual);
  const linear = fitLinear(aligned.xs, aligned.ys);

  const shapePredicted = new Map<string, number>();
  const shapeSpot = new Map<string, number>();
  for (let q = 0; q < 3 * 96 + 17; q++) {
    shapePredicted.set(
      quarterKey(ANCHOR + 6 * HOUR_MS + q * QUARTER_MS),
      rand() * 20 - 5,
    );
  }
  for (let q = -8 * 96; q < 2 * 96; q++) {
    if (rand() < 0.7)
      shapeSpot.set(
        quarterKey(ANCHOR + q * QUARTER_MS),
        round(rand() * 20 - 5, 3),
      );
  }
  const shaped = applyWithinDayShape(shapePredicted, shapeSpot);

  const roundingValues = [
    1.0005, 2.0004999999999997, -0.0005, -0.0004, -1.2345, 0.0015, 1e-4,
    -2.5e-3, 123.4565, -123.4565, 0.49999999999999994, 7, -0, 1e-12, -1e-12,
    5.0000000001,
  ];
  const roundingPredicted = new Map<string, number>();
  roundingValues.forEach((v, i) => {
    if (i !== 3 && i !== 9)
      roundingPredicted.set(quarterKey(ANCHOR + (i + 2) * QUARTER_MS), v);
  });
  const predictedSeries = buildPredictedSeries(
    roundingPredicted,
    ANCHOR,
    roundingValues.length + 4,
  );

  const sanityCases = [
    new Map<string, number>(),
    toMap([ANCHOR, ANCHOR + QUARTER_MS], [4, 4]),
    toMap(
      [ANCHOR, ANCHOR + QUARTER_MS, ANCHOR + 2 * QUARTER_MS],
      [2, 11.5, -3.25],
    ),
    toMap([ANCHOR, ANCHOR + QUARTER_MS], [40, 45]),
  ].map((m) => ({
    history: mapEntries(m),
    expected: sanityBoundFromHistory(m),
  }));

  return {
    quarterKeys,
    bucket: {
      input: bucketInput,
      expected: mapEntries(bucketRecords(bucketRecs)),
    },
    expandHourly: {
      input: hourlyInput,
      expected: mapEntries(
        expandHourlyToQuarters(records(hourlyInput, [-Infinity, Infinity])),
      ),
    },
    extend: extendCases,
    align: {
      price: mapEntries(alignPrice),
      residual: mapEntries(alignResidual),
      xs: aligned.xs,
      ys: aligned.ys,
      slope: linear.slope,
      intercept: linear.intercept,
    },
    shape: {
      predicted: mapEntries(shapePredicted),
      spot: mapEntries(shapeSpot),
      expected: mapEntries(shaped.result),
      shapeFallbackQuarters: shaped.shapeFallbackQuarters,
    },
    predictedSeries: {
      predicted: mapEntries(roundingPredicted),
      seriesStartMs: ANCHOR,
      numQuarters: roundingValues.length + 4,
      estimate: predictedSeries.series.map((p) => p.estimatedSpotCentsKwh),
      filledQuarters: predictedSeries.filledQuarters,
      zeroSeededQuarters: predictedSeries.zeroSeededQuarters,
    },
    zeroSeeded: (() => {
      const built = buildPredictedSeries(new Map(), ANCHOR, 3);
      return {
        estimate: built.series.map((p) => p.estimatedSpotCentsKwh),
        zeroSeededQuarters: built.zeroSeededQuarters,
      };
    })(),
    sanity: sanityCases,
  };
};

const featureUnits = (): unknown => {
  const s = scenarios[0];
  if (s === undefined) throw new Error("no scenario");
  const spot = runMap(dataset.fi, [
    BASE_START_MS,
    BASE_START_MS + BASE_QUARTERS * QUARTER_MS,
  ]);
  const neighbours = new Map<string, Map<string, number>>([
    [
      "SE1",
      runMap(dataset.se1, [BASE_START_MS, at("2026-03-20T00:00:00.000Z")]),
    ],
    [
      "EE",
      runMap(dataset.ee, [
        BASE_START_MS,
        BASE_START_MS + BASE_QUARTERS * QUARTER_MS,
      ]),
    ],
  ]);
  const all: Range = [-Infinity, Infinity];
  const windByKey = bucketRecords(records(dataset.windActual, all));
  const consumptionByKey = expandHourlyToQuarters(
    records(dataset.consActual, all),
  );
  const ctx: FeatureContext = {
    fiPricesByKey: spot,
    neighborPricesByArea: neighbours,
    windByKey,
    consumptionByKey,
  };
  const instants = [
    "2026-03-04T12:00:00.000Z",
    "2026-03-07T08:00:00.000Z",
    "2026-03-08T23:45:00.000Z",
    "2026-03-14T00:00:00.000Z",
    "2026-03-03T10:00:00.000Z",
    "2026-02-19T12:00:00.000Z",
    "2026-02-20T00:00:00.000Z",
    "2026-03-16T10:15:00.000Z",
    "2026-04-12T06:30:00.000Z",
    "2026-03-25T00:00:00.000Z",
  ].map((iso) => Date.parse(iso));
  const vectors = instants.map((ms) => buildFeatureVector(ms, ctx));
  const origin = at("2026-03-18T13:00:00.000Z");
  const training = buildTrainingMatrix(origin, ctx);
  const n = training.targets.length;
  const sampleIndices = [0, 1, 95, Math.floor(n / 2), n - 1];
  return {
    featureNames: FEATURE_NAMES,
    neighbourRanges: {
      SE1: [BASE_START_MS, at("2026-03-20T00:00:00.000Z")],
      EE: [BASE_START_MS, BASE_START_MS + BASE_QUARTERS * QUARTER_MS],
    },
    instantsMs: instants,
    vectors,
    training: {
      originMs: origin,
      count: n,
      firstKeyMs: Date.parse(training.keys[0] ?? ""),
      lastKeyMs: Date.parse(training.keys[n - 1] ?? ""),
      sampleIndices,
      sampleRows: sampleIndices.map((i) => training.features.rows[i] ?? []),
      sampleTargets: sampleIndices.map((i) => training.targets[i] ?? 0),
    },
  };
};

interface ModelCase {
  readonly name: string;
  readonly featureCount: number;
  readonly rows: number[][];
  readonly targets: number[];
  readonly options: FitOptions;
  readonly probes: number[][];
}

const modelUnits = (): unknown => {
  const rand = prng(4242);
  const cases: ModelCase[] = [];
  const randomRows = (n: number, p: number): number[][] =>
    Array.from({ length: n }, () =>
      Array.from({ length: p }, (_u, j) =>
        round((rand() - 0.3) * 10 ** (j % 4), 4),
      ),
    );
  const base = randomRows(120, 5);
  const linearTargets = base.map(
    (r) =>
      2 +
      0.5 * (r[0] ?? 0) -
      0.03 * (r[1] ?? 0) +
      0.002 * (r[2] ?? 0) +
      (rand() - 0.5) * 0.3,
  );
  const probes = [...base.slice(0, 5), ...randomRows(5, 5), [0, 0, 0, 0, 0]];
  for (const lambda of [0, 0.5, 1, 100]) {
    for (const transform of ["none", "arcsinh"] as const) {
      cases.push({
        name: `random_l${String(lambda)}_${transform}`,
        featureCount: 5,
        rows: base,
        targets: linearTargets,
        options: { ridgeLambda: lambda, priceTransform: transform },
        probes,
      });
    }
  }
  const constCol = base.map((r) => [r[0] ?? 0, 42, r[1] ?? 0]);
  cases.push({
    name: "constant_column",
    featureCount: 3,
    rows: constCol,
    targets: linearTargets,
    options: { ridgeLambda: 0.0001, priceTransform: "none" },
    probes: constCol.slice(0, 4),
  });
  cases.push({
    name: "too_few_samples",
    featureCount: 1,
    rows: Array.from({ length: 23 }, (_u, i) => [i]),
    targets: Array.from({ length: 23 }, (_u, i) => i * 2),
    options: {},
    probes: [[10]],
  });
  cases.push({
    name: "zero_features",
    featureCount: 0,
    rows: Array.from({ length: 50 }, () => []),
    targets: Array.from({ length: 50 }, () => 1),
    options: {},
    probes: [[]],
  });
  cases.push({
    name: "singular",
    featureCount: 2,
    rows: Array.from({ length: 40 }, (_u, i) => [i, i]),
    targets: Array.from({ length: 40 }, (_u, i) => i + 1),
    options: { ridgeLambda: 0, priceTransform: "none" },
    probes: [[3, 3]],
  });
  cases.push({
    name: "short_rows_and_extra_targets",
    featureCount: 3,
    rows: base
      .slice(0, 40)
      .map((r, i) => (i % 5 === 0 ? r.slice(0, 2) : r.slice(0, 3))),
    targets: linearTargets.slice(0, 45),
    options: {},
    probes: [
      [1, 2],
      [1, 2, 3, 4],
    ],
  });
  return cases.map((c) => {
    const names = Array.from(
      { length: c.featureCount },
      (_u, j) => `f${String(j)}`,
    );
    const fitted = createRidgeModel().fit(
      { features: { featureNames: names, rows: c.rows }, targets: c.targets },
      c.options,
    );
    return {
      ...c,
      options: {
        ridgeLambda: c.options.ridgeLambda ?? null,
        priceTransform: c.options.priceTransform ?? null,
      },
      meta: fitted.meta,
      predictions: c.probes.map((x) => fitted.predict(x)),
    };
  });
};

const conformalUnits = (): unknown => {
  const rand = prng(99);
  const residuals: HorizonResidual[] = [];
  for (let i = 0; i < 600; i++) {
    const hour = Math.floor(rand() * 24);
    // Hour 5 is thin: most of its samples are dropped.
    if (hour === 5 && rand() < 0.9) continue;
    const predictedRaw = round((rand() - 0.2) * 20, 3);
    const r = (rand() - 0.5) * (hour % 3 === 0 ? 0.4 : 1.6);
    residuals.push({
      utcHour: hour,
      predictedRaw,
      actualRaw: round(Math.sinh(Math.asinh(predictedRaw) + r), 3),
    });
  }
  const derived = deriveBandOffsets(residuals);
  const derivedWide = deriveBandOffsets(residuals, {
    nominalCoverage: 0.5,
    minHourSamples: 30,
  });
  const asArray = (
    m: ReadonlyMap<number, BandOffsets>,
  ): (BandOffsets | null)[] =>
    Array.from({ length: 24 }, (_u, h) => m.get(h) ?? null);
  // Tight own-hour buckets (hours 0–11) and wide thin hours (12–23) that use
  // the global offsets: the measured coverage falls below the gate.
  const belowGate: HorizonResidual[] = [];
  for (let hour = 0; hour < 24; hour++) {
    const n = hour < 12 ? 30 : 13;
    const spread = hour < 12 ? 0.01 : 3;
    for (let i = 0; i < n; i++) {
      const predictedRaw = round((rand() - 0.2) * 20, 3);
      const r = (rand() * 2 - 1) * spread;
      belowGate.push({
        utcHour: hour,
        predictedRaw,
        actualRaw: round(Math.sinh(Math.asinh(predictedRaw) + r), 3),
      });
    }
  }
  const artifacts = {
    tooFew: buildArtifact(residuals.slice(0, 95), "2026-06-01T00:00:00.000Z"),
    calibrated: buildArtifact(residuals, "2026-06-01T00:00:00.000Z"),
    belowGate: buildArtifact(belowGate, "2026-06-01T00:00:00.000Z"),
  };
  const artifactJson = (a: CalibratedBands): unknown => ({
    observedCoverage: a.observedCoverage,
    calibrated: a.calibrated,
    offsetsByHour: asArray(a.offsetsByHour),
    globalOffsets: a.globalOffsets,
    generatedAt: a.generatedAt,
  });
  const points = [-30, -3, -1, -0.0004, 0, 0.0004, 0.5, 3.333, 5, 12.75, 250];
  const applied = [0, 5, 7, 12, 18, 23].map((hour) => ({
    hour,
    bands: points.map((p) => applyBand(p, hour, STUB_BANDS)),
  }));
  return {
    residuals: {
      utcHour: residuals.map((r) => r.utcHour),
      predictedRaw: residuals.map((r) => r.predictedRaw),
      actualRaw: residuals.map((r) => r.actualRaw),
    },
    derived: {
      offsetsByHour: asArray(derived.offsetsByHour),
      globalOffsets: derived.globalOffsets,
      coverage: observedCoverageOf(
        residuals,
        derived.offsetsByHour,
        derived.globalOffsets,
      ),
    },
    derivedWide: {
      offsetsByHour: asArray(derivedWide.offsetsByHour),
      globalOffsets: derivedWide.globalOffsets,
      coverage: observedCoverageOf(
        residuals,
        derivedWide.offsetsByHour,
        derivedWide.globalOffsets,
      ),
    },
    belowGateResiduals: {
      utcHour: belowGate.map((r) => r.utcHour),
      predictedRaw: belowGate.map((r) => r.predictedRaw),
      actualRaw: belowGate.map((r) => r.actualRaw),
    },
    artifacts: {
      tooFew: artifactJson(artifacts.tooFew),
      calibrated: artifactJson(artifacts.calibrated),
      belowGate: artifactJson(artifacts.belowGate),
    },
    stubBands: artifactJson(STUB_BANDS),
    applyPoints: points,
    applied,
  };
};

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

const write = async (name: string, value: unknown): Promise<void> => {
  const text = await format(JSON.stringify(value), { parser: "json" });
  writeFileSync(path.join(outDir, name), text);
};

mkdirSync(outDir, { recursive: true });
await write("dataset.json", { baseStartMs: BASE_START_MS, ...dataset });
await write("scenarios.json", scenarios.map(runScenario));
await write("series.json", seriesUnits());
await write("features.json", featureUnits());
await write("model.json", modelUnits());
await write("conformal.json", conformalUnits());

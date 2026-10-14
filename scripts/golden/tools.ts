/**
 * Writes the golden fixtures of the Rust forecast tools to
 * `crates/tools/tests/golden/`. Run with
 * `node --import tsx scripts/golden/tools.ts`.
 *
 * The script imports the real TypeScript tools (`tools/*.ts`). It builds
 * deterministic data sets with a seeded generator, writes the inputs, and
 * writes the outputs of the TypeScript functions for these inputs. It runs
 * the real CLIs (`tools/backtest-cli.ts`, `tools/revision-magnitude-cli.ts`)
 * as child processes and keeps their stdout, stderr, and exit status. It
 * never runs `tools/regenerate-bands.ts`, because that script rewrites
 * `src/conformal-artifact.ts`; it calls the same exported functions instead.
 * The Rust tests read the inputs; they never run a generator.
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { format } from "prettier";
import {
  compareOptimism,
  deriveBandsFromBacktest,
  loadFixture,
  runBacktest,
  type BacktestData,
  type BacktestSummary,
  type PricePoint,
} from "../../tools/backtest.js";
import { toFixtureJson as backtestFixtureJson } from "../../tools/backtest-cli.js";
import {
  bias,
  mae,
  median,
  percentile,
  precisionAtN,
  rms,
  rmse,
  spearman,
  standardDeviation,
} from "../../tools/backtest-metrics.js";
import { renderArtifact } from "../../tools/regenerate-bands.js";
import {
  recommendation,
  runRevisionStudy,
  type ActualRecord,
  type RevisionStudyInput,
} from "../../tools/revision-magnitude.js";
import { toFixtureJson as vintageFixtureJson } from "../../tools/revision-magnitude-cli.js";
import type { FingridRecord, ForecastVintageRecord } from "../../src/types.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const GENERATED_AT = "2026-10-14T00:00:00.000Z";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, "..", "..");
const outDir = path.join(root, "crates", "tools", "tests", "golden");
const committedFixture = path.join(
  root,
  "tools",
  "backtest-data",
  "fixture.json",
);

const tmp = mkdtempSync(path.join(tmpdir(), "golden-tools-"));

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

const r3 = (x: number): number => Math.round(x * 1000) / 1000;
const iso = (ms: number): string => new Date(ms).toISOString();

// ---------------------------------------------------------------------------
// Backtest data set: 94 days, so the scored window passes the 90-day guard
// ---------------------------------------------------------------------------

const BACKTEST_START_MS = Date.parse("2026-01-05T00:00:00.000Z");
const BACKTEST_DAYS = 94;
const AREAS = ["FI", "SE1", "SE3", "EE"] as const;
/** Issuance leads of the 245/165 vintage ladder: a fresh and a stale one. */
const LADDER_LEADS_H = [2, 130] as const;
/** FI quarters with no price: a gap that exercises forward fill and baselines. */
const FI_GAP = { fromQ: 40 * 96 + 30, toQ: 40 * 96 + 54 };

interface BacktestColumns {
  readonly startMs: number;
  readonly quarters: number;
  /** Per area, the spot c/kWh per quarter; null means no price. */
  readonly prices: Record<string, (number | null)[]>;
  readonly actuals: Record<string, number[]>;
  /** Per forecast dataset, one value column per ladder lead. */
  readonly vintages: Record<string, { leadH: number; values: number[] }[]>;
}

const buildBacktestColumns = (): BacktestColumns => {
  const rand = prng(20261014);
  const quarters = BACKTEST_DAYS * 96;
  const prices: Record<string, (number | null)[]> = {
    FI: [],
    SE1: [],
    SE3: [],
    EE: [],
  };
  const wind75: number[] = [];
  const cons124: number[] = [];
  const wind245: number[][] = [[], []];
  const cons165: number[][] = [[], []];
  for (let q = 0; q < quarters; q++) {
    const day = Math.floor(q / 96);
    const weekend = day % 7 >= 5;
    const consumption =
      8000 + (q % 96) * 15 + Math.round(rand() * 400) - (weekend ? 300 : 0);
    const windMw = 2000 + ((q * 131) % 1800) + Math.round(rand() * 300);
    // Every tenth day has a low price level, so some prices are negative.
    const level = day % 10 === 9 ? -8 : 0;
    const fi = r3(0.0012 * (consumption - windMw) + 2 + level + (rand() - 0.5));
    const inGap = q >= FI_GAP.fromQ && q < FI_GAP.toQ;
    prices.FI?.push(inGap ? null : fi);
    prices.SE1?.push(r3(fi - 0.5 + (rand() - 0.5) * 0.2));
    prices.SE3?.push(r3(fi + 0.3 + (rand() - 0.5) * 0.2));
    prices.EE?.push(r3(fi + 0.1 + (rand() - 0.5) * 0.2));
    wind75.push(windMw);
    cons124.push(consumption);
    wind245[0]?.push(windMw);
    wind245[1]?.push(windMw + Math.round((rand() - 0.5) * 3000));
    cons165[0]?.push(consumption);
    cons165[1]?.push(consumption + Math.round((rand() - 0.5) * 600));
  }
  return {
    startMs: BACKTEST_START_MS,
    quarters,
    prices,
    actuals: { "75": wind75, "124": cons124 },
    vintages: {
      "245": LADDER_LEADS_H.map((leadH, i) => ({
        leadH,
        values: wind245[i] ?? [],
      })),
      "165": LADDER_LEADS_H.map((leadH, i) => ({
        leadH,
        values: cons165[i] ?? [],
      })),
    },
  };
};

/** The backtest data of the columns; the Rust tests build the same. */
const backtestDataOf = (c: BacktestColumns): BacktestData => {
  const prices: PricePoint[] = [];
  for (const area of AREAS) {
    (c.prices[area] ?? []).forEach((value, q) => {
      if (value !== null) {
        prices.push({
          area,
          start: iso(c.startMs + q * QUARTER_MS),
          spotCentsKwh: value,
        });
      }
    });
  }
  const fingridActualsByDataset: Record<string, FingridRecord[]> = {};
  for (const [id, values] of Object.entries(c.actuals)) {
    fingridActualsByDataset[id] = values.map((value, q) => {
      const ms = c.startMs + q * QUARTER_MS;
      return {
        datasetId: Number(id),
        startTime: iso(ms),
        endTime: iso(ms + QUARTER_MS),
        value,
      };
    });
  }
  const fingridForecastVintagesByDataset: Record<
    string,
    ForecastVintageRecord[]
  > = {};
  for (const [id, ladder] of Object.entries(c.vintages)) {
    const records: ForecastVintageRecord[] = [];
    for (let q = 0; q < c.quarters; q++) {
      const ms = c.startMs + q * QUARTER_MS;
      for (const rung of ladder) {
        records.push({
          datasetId: Number(id),
          issuedAt: iso(ms - rung.leadH * HOUR_MS),
          startTime: iso(ms),
          endTime: iso(ms + QUARTER_MS),
          value: rung.values[q] ?? 0,
        });
      }
    }
    fingridForecastVintagesByDataset[id] = records;
  }
  return { prices, fingridActualsByDataset, fingridForecastVintagesByDataset };
};

const summaryJson = (s: BacktestSummary, withResiduals: boolean): unknown => ({
  origins: s.origins,
  fallbackOrigins: s.fallbackOrigins,
  preVintageOrigins: s.preVintageOrigins,
  scoredWindowStart: s.scoredWindowStart,
  scoredWindowEnd: s.scoredWindowEnd,
  scoredWindowSpanDays: s.scoredWindowSpanDays,
  scoredOriginsMs: [...s.scoredOriginsMs],
  modelMae: s.modelMae,
  modelSmape: s.modelSmape,
  baselineMae: s.baselineMae,
  rMae: s.rMae,
  leakFree: s.leakFree,
  byHorizon: s.byHorizon,
  residualCount: s.residuals.length,
  residualHours: withResiduals ? s.residuals.map((r) => r.utcHour) : null,
  residualPredMilli: withResiduals
    ? s.residuals.map((r) => Math.round(r.predictedRaw * 1000))
    : null,
});

interface CliRun {
  readonly args: string[];
  readonly stdout: string;
  readonly stderr: string;
  readonly status: number | null;
}

/**
 * Writes a module hook that resolves `pg` to a fake. The fake answers the
 * three `--db` reads with the rows of `GOLDEN_FAKE_PG_ROWS`, which are
 * already in the `ORDER BY` order of the SQL. It ignores the time window;
 * `crates/tools/tests/db.rs` checks the window against PostgreSQL.
 */
const fakePgRegister = (): string => {
  writeFileSync(
    path.join(tmp, "fake-pg.mjs"),
    `import { readFileSync } from "node:fs";
const rows = JSON.parse(readFileSync(process.env.GOLDEN_FAKE_PG_ROWS, "utf8"));
class Pool {
  async query(sql, params) {
    if (sql.includes("FROM prices")) {
      return { rows: rows.prices.filter((r) => params[0].includes(r.area)) };
    }
    if (sql.includes("FROM fingrid_actuals")) {
      return { rows: rows.actuals[String(params[0])] ?? [] };
    }
    if (sql.includes("FROM fingrid_forecasts")) {
      return { rows: rows.vintages[String(params[0])] ?? [] };
    }
    throw new Error("unexpected query: " + sql);
  }
  async end() {}
}
export default { Pool, types: { setTypeParser() {} } };
`,
  );
  // A synchronous hook, like the one of tsx; an off-thread `register` hook
  // stops the script guard of the CLI from matching.
  const register = path.join(tmp, "fake-pg-register.mjs");
  writeFileSync(
    register,
    `import { registerHooks } from "node:module";
const fake = new URL("./fake-pg.mjs", import.meta.url).href;
registerHooks({
  resolve(specifier, context, next) {
    return specifier === "pg" ? { url: fake, shortCircuit: true } : next(specifier, context);
  },
});
`,
  );
  return pathToFileURL(register).href;
};

/**
 * The database rows of `data`, in the order of the store SQL. A price is
 * stored as EUR/MWh (`spot × 10`); the CLI converts it back exactly.
 */
const fakeDbRows = (data: BacktestData): unknown => {
  const byAreaThenStart = (a: PricePoint, b: PricePoint): number =>
    a.area < b.area
      ? -1
      : a.area > b.area
        ? 1
        : Date.parse(a.start) - Date.parse(b.start);
  const prices = [...data.prices].sort(byAreaThenStart).map((p) => ({
    area: p.area,
    delivery_start: p.start,
    delivery_end: iso(Date.parse(p.start) + QUARTER_MS),
    price_eur_mwh: p.spotCentsKwh * 10,
  }));
  const actuals = Object.fromEntries(
    Object.entries(data.fingridActualsByDataset).map(([id, records]) => [
      id,
      records.map((r) => ({
        dataset_id: r.datasetId,
        start_time: r.startTime,
        end_time: r.endTime,
        value: r.value,
      })),
    ]),
  );
  const vintages = Object.fromEntries(
    Object.entries(data.fingridForecastVintagesByDataset).map(
      ([id, records]) => [
        id,
        [...records]
          .sort(
            (a, b) =>
              Date.parse(a.startTime) - Date.parse(b.startTime) ||
              Date.parse(a.issuedAt) - Date.parse(b.issuedAt),
          )
          .map((r) => ({
            dataset_id: r.datasetId,
            issued_at: r.issuedAt,
            start_time: r.startTime,
            end_time: r.endTime,
            value: r.value,
          })),
      ],
    ),
  );
  return { prices, actuals, vintages };
};

/** Replaces local paths, so the golden files do not depend on the machine. */
const scrub = (text: string): string =>
  text.replaceAll(tmp, "<tmp>").replaceAll(root, "<root>");

/**
 * Runs a TypeScript CLI as a child process; `args` follow the script path.
 * With `fakeDb`, the module `pg` is replaced by an in-memory fake (below)
 * that serves the rows of that file.
 */
const runCli = (script: string, args: string[], fakeDb?: string): CliRun => {
  const imports = fakeDb === undefined ? [] : ["--import", fakePgRegister()];
  const env: Record<string, string> = { PATH: process.env.PATH ?? "" };
  if (fakeDb !== undefined) {
    env.DATABASE_PUBLIC_URL = "postgres://fake";
    env.GOLDEN_FAKE_PG_ROWS = fakeDb;
  }
  const r = spawnSync(
    process.execPath,
    ["--import", "tsx", ...imports, script, ...args],
    { cwd: root, encoding: "utf8", env },
  );
  // A thrown error prints a Node stack trace; keep only its message.
  const stderr = r.stderr.includes("\n    at ")
    ? r.stderr
        .split("\n")
        .filter((line) => line.startsWith("Error:"))
        .join("\n")
    : r.stderr;
  return {
    args: args.map(scrub),
    stdout: scrub(r.stdout),
    stderr: scrub(stderr),
    status: r.status,
  };
};

const bandsJson = (data: BacktestData): unknown => {
  const { summary, bands, observedCoverage } = deriveBandsFromBacktest(
    data,
    GENERATED_AT,
  );
  const provenance = {
    scoredWindowStart: summary.scoredWindowStart,
    scoredWindowEnd: summary.scoredWindowEnd,
    scoredOrigins: summary.origins,
    scoredWindowSpanDays: summary.scoredWindowSpanDays,
    observedCoverage,
  };
  return {
    calibrated: bands.calibrated,
    nominalCoverage: bands.nominalCoverage,
    observedCoverage: bands.observedCoverage,
    measuredCoverage: observedCoverage,
    offsetsByHour: [...bands.offsetsByHour.entries()].sort(
      (a, b) => a[0] - b[0],
    ),
    globalOffsets: bands.globalOffsets,
    generatedAt: bands.generatedAt,
    provenance,
    residualCount: summary.residuals.length,
    tsModule: renderArtifact(bands, provenance),
  };
};

const backtestColumns = buildBacktestColumns();
const synthetic = backtestDataOf(backtestColumns);
const syntheticFile = path.join(tmp, "synthetic.json");
writeFileSync(
  syntheticFile,
  JSON.stringify(JSON.parse(backtestFixtureJson(synthetic))),
);
const fakeDbFile = path.join(tmp, "fake-db.json");
writeFileSync(fakeDbFile, JSON.stringify(fakeDbRows(synthetic)));
const committed = loadFixture(committedFixture);

const cmp = compareOptimism(synthetic);
const smallData: BacktestData = {
  prices: synthetic.prices.filter(
    (p) => Date.parse(p.start) < BACKTEST_START_MS + HOUR_MS,
  ),
  fingridActualsByDataset: Object.fromEntries(
    Object.entries(synthetic.fingridActualsByDataset).map(([k, v]) => [
      k,
      v.slice(0, 2),
    ]),
  ),
  fingridForecastVintagesByDataset: Object.fromEntries(
    Object.entries(synthetic.fingridForecastVintagesByDataset).map(([k, v]) => [
      k,
      v.slice(0, 3),
    ]),
  ),
};

const backtestExpected = {
  generatedAt: GENERATED_AT,
  synthetic: {
    summary: summaryJson(runBacktest(synthetic), true),
    compare: {
      scoredOrigins: cmp.scoredOrigins,
      deltaMae: cmp.deltaMae,
      deltaSmape: cmp.deltaSmape,
      deltaBandCoverage: cmp.deltaBandCoverage,
      deltaByHorizon: cmp.deltaByHorizon,
      ladderDiagnostic: Object.entries(cmp.ladderDiagnostic),
      leaked: summaryJson(cmp.leaked, false),
      honestLeakFree: cmp.honest.leakFree,
    },
    bands: bandsJson(synthetic),
    cli: runCli("tools/backtest-cli.ts", ["--data", syntheticFile]),
  },
  committedFixture: {
    summary: summaryJson(runBacktest(committed), true),
    bands: bandsJson(committed),
    cli: runCli("tools/backtest-cli.ts", ["--data", committedFixture]),
  },
  dbCompare: runCli("tools/backtest-cli.ts", ["--db", "--compare"], fakeDbFile),
  usage: runCli("tools/backtest-cli.ts", []),
  badWindow: runCli("tools/backtest-cli.ts", ["--db", "--window", "abc"]),
  fixtureJson: { input: smallData, text: backtestFixtureJson(smallData) },
};

// ---------------------------------------------------------------------------
// Revision study cases
// ---------------------------------------------------------------------------

interface VintageColumns {
  issuedMs: number[];
  startMs: number[];
  value: number[];
}
interface ActualColumns {
  startMs: number[];
  value: number[];
}
interface RevisionCase {
  name: string;
  vintages: Record<string, VintageColumns>;
  actuals: Record<string, ActualColumns>;
}

const REVISION_BASE_MS = Date.parse("2026-06-25T00:00:00.000Z");

const pushVintage = (
  c: VintageColumns,
  targetMs: number,
  leadH: number,
  value: number,
): void => {
  c.issuedMs.push(targetMs - leadH * HOUR_MS);
  c.startMs.push(targetMs);
  c.value.push(value);
};

/** The unit-test ladder: one post-delivery reference and constant revisions. */
const genCase = (
  name: string,
  datasetId: string,
  n: number,
  refBase: number,
  refStep: number,
  revision: number,
): RevisionCase => {
  const c: VintageColumns = { issuedMs: [], startMs: [], value: [] };
  for (let t = 0; t < n; t++) {
    const target = REVISION_BASE_MS + t * 6 * HOUR_MS;
    const ref = refBase + t * refStep;
    pushVintage(c, target, -1, ref);
    for (const lead of [6, 12, 24, 36]) {
      pushVintage(c, target, lead, ref + revision);
    }
  }
  return { name, vintages: { [datasetId]: c }, actuals: {} };
};

/** Noisy ladders with stale, pre-delivery, and future targets and actuals. */
const mixedCase = (): RevisionCase => {
  const rand = prng(79);
  const wind: VintageColumns = { issuedMs: [], startMs: [], value: [] };
  const cons: VintageColumns = { issuedMs: [], startMs: [], value: [] };
  const windActual: ActualColumns = { startMs: [], value: [] };
  const consActual: ActualColumns = { startMs: [], value: [] };
  const targets = 150;
  for (let t = 0; t < targets; t++) {
    const target = REVISION_BASE_MS + t * 2 * HOUR_MS;
    const w = 2000 + ((t * 131) % 1500) + Math.round(rand() * 200);
    const c = 8000 + (t % 12) * 120 + Math.round(rand() * 100);
    const kind = t % 20;
    // Most references are post-delivery; some are near-delivery or stale.
    const refLead = kind === 3 ? 1 : kind === 7 ? 4 : -1;
    pushVintage(wind, target, refLead, w);
    pushVintage(cons, target, refLead, c);
    for (const lead of [3, 9, 15, 21, 30, 42, 60]) {
      pushVintage(
        wind,
        target,
        lead,
        w + Math.round((rand() - 0.5) * lead * 40),
      );
    }
    for (const lead of [3, 9, 15, 21]) {
      pushVintage(
        cons,
        target,
        lead,
        c + Math.round((rand() - 0.5) * lead * 6),
      );
    }
    windActual.startMs.push(target);
    windActual.value.push(w + Math.round((rand() - 0.5) * 50));
    if (t % 2 === 0) {
      consActual.startMs.push(target);
      consActual.value.push(c + Math.round((rand() - 0.5) * 20));
    }
  }
  // Future targets: delivered after the last issuance.
  const lastTarget = REVISION_BASE_MS + (targets - 1) * 2 * HOUR_MS;
  for (const offsetH of [30, 40]) {
    const target = lastTarget + offsetH * HOUR_MS;
    pushVintage(wind, target, 12, 2500);
    pushVintage(wind, target, 24, 2600);
  }
  return {
    name: "mixed",
    vintages: { "245": wind, "165": cons },
    actuals: { "75": windActual, "124": consActual },
  };
};

const studyInputOf = (c: RevisionCase): RevisionStudyInput => {
  const vintagesByDataset: Record<string, ForecastVintageRecord[]> = {};
  for (const [id, col] of Object.entries(c.vintages)) {
    vintagesByDataset[id] = col.startMs.map((startMs, i) => ({
      datasetId: Number(id),
      issuedAt: iso(col.issuedMs[i] ?? 0),
      startTime: iso(startMs),
      endTime: iso(startMs + QUARTER_MS),
      value: col.value[i] ?? 0,
    }));
  }
  const actualsByDataset: Record<string, ActualRecord[]> = {};
  for (const [id, col] of Object.entries(c.actuals)) {
    actualsByDataset[id] = col.startMs.map((startMs, i) => ({
      datasetId: Number(id),
      startTime: iso(startMs),
      value: col.value[i] ?? 0,
    }));
  }
  return { vintagesByDataset, actualsByDataset };
};

const revisionCases: RevisionCase[] = [
  mixedCase(),
  genCase("go", "245", 150, 1000, 20, 2000),
  genCase("marginal_provisional", "165", 150, 8000, 50, 1),
  genCase("marginal_modest", "245", 150, 1000, 20, 250),
  genCase("defer", "245", 50, 1000, 20, 2000),
  { name: "empty", vintages: {}, actuals: {} },
];

const revisionExpected = revisionCases.map((c) => {
  const input = studyInputOf(c);
  const file = path.join(tmp, `${c.name}.json`);
  writeFileSync(file, vintageFixtureJson(input));
  const result = runRevisionStudy(input);
  return {
    name: c.name,
    result,
    recommendation: recommendation(result),
    cli: runCli("tools/revision-magnitude-cli.ts", ["--data", file]),
    fixtureJson: c.name === "defer" ? vintageFixtureJson(input) : null,
  };
});

// ---------------------------------------------------------------------------
// Metrics cases
// ---------------------------------------------------------------------------

const metricsRand = prng(7);
const series = (n: number, ties: boolean): number[] =>
  Array.from({ length: n }, () =>
    ties ? Math.floor(metricsRand() * 5) : r3(metricsRand() * 20 - 5),
  );
const metricsCases = [
  { pred: [1, 2, 3], act: [2, 2, 5] },
  { pred: [4, 1, 8, 3], act: [5, 1, 9, 2] },
  { pred: [0, -0, 0], act: [1, 2, 3] },
  { pred: series(17, true), act: series(17, true) },
  { pred: series(40, false), act: series(40, false) },
  { pred: series(96, false), act: series(96, true) },
].map(({ pred, act }) => ({
  pred,
  act,
  mae: mae(pred, act),
  rmse: rmse(pred, act),
  bias: bias(pred, act),
  spearman: spearman(pred, act),
  precisionCheap: [1, 2, 16, 200].map((n) =>
    precisionAtN(pred, act, n, "cheap"),
  ),
  precisionPeak: [1, 2, 16, 200].map((n) => precisionAtN(pred, act, n, "peak")),
  median: median(pred),
  percentiles: [0, 10, 50, 90, 99.5, 100].map((p) => percentile(act, p)),
  standardDeviation: standardDeviation(pred),
  rms: rms(act),
}));

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

const write = async (name: string, value: unknown): Promise<void> => {
  const text = await format(JSON.stringify(value), { parser: "json" });
  writeFileSync(path.join(outDir, name), text);
};

mkdirSync(outDir, { recursive: true });
await write("backtest_input.json", backtestColumns);
await write("backtest_expected.json", backtestExpected);
await write("revision_input.json", revisionCases);
await write("revision_expected.json", revisionExpected);
await write("metrics.json", metricsCases);
rmSync(tmp, { recursive: true, force: true });

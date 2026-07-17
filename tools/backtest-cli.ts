/**
 * OFFLINE / DEV-ONLY backtest CLI for the FI forecast.
 *
 * IMPORTANT: this module lives in `tools/` (offline-only). It must NOT be
 * imported by any `src/` runtime module — the production bundle's only tsup
 * entry is `src/index.ts`, and an ESLint guard forbids `src/` runtime from
 * importing `tools/`, so this file never reaches `dist/`. It runs only as a tsx
 * script:
 *
 *     pnpm backtest --data tools/backtest-data/fixture.json   # replay a fixture
 *     pnpm backtest --db                            # score against the live DB
 *     pnpm backtest --db --window 120               # widen the DB window (days)
 *     pnpm backtest --db --export /tmp/snapshot.json   # snapshot DB → fixture
 *     pnpm backtest --db --compare                  # vintage-leak optimism (#80)
 *
 * It is the single operator entry point for measuring forecast accuracy on
 * demand: it runs the EXISTING pure `runBacktest` (issue-time, leakage-guarded)
 * over real data and prints MAE / rMAE-vs-naive / sMAPE + band coverage. All DB
 * I/O lives here; `backtest.ts` stays pure. This is NOT a scheduled job and adds
 * no endpoint (`STACK §9`).
 *
 * It reads `process.argv` and (for `--db`) the connection string via `env.ts`;
 * it never logs the connection string or any secret (`STACK §8`).
 */
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
// Import db.js FIRST for its TIMESTAMPTZ→ISO type-parser side-effect (db.ts):
// without it `pg` returns TIMESTAMPTZ columns as `Date` objects, which would
// diverge from the ISO strings the fixture path produces and break parity.
import { closeDatabase } from "../src/db.js";
import pg from "pg";
import { loadEnv } from "../src/env.js";
import {
  compareOptimism,
  HORIZON_LABELS,
  loadFixture,
  PRECISION_N_QUARTERS,
  runBacktest,
  type BacktestData,
  type OptimismComparison,
  type PricePoint,
} from "./backtest.js";
import { deriveBandOffsets, observedCoverageOf } from "../src/conformal.js";
import { getPricesByAreas } from "../src/price-store.js";
import {
  getFingridForecastVintagesAll,
  getFingridRecordsByRange,
} from "../src/fingrid-store.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "../src/fingrid.js";
import { eurMwhToCentsKwh } from "../src/nordpool.js";
import type {
  FingridRecord,
  ForecastVintageRecord,
  HourlyPrice,
} from "../src/types.js";

const { Pool } = pg;

const DAY_MS = 24 * 60 * 60 * 1000;

/** Default DB scoring window (days back from now). */
export const DEFAULT_WINDOW_DAYS = 90;

/**
 * Minimum scoreable origins before the metrics are treated as a verdict. Below
 * this the CLI warns loudly and exits non-zero so a wrapper cannot read thin
 * data as a pass.
 */
export const MIN_SCOREABLE_ORIGINS = 14;

/** The areas scored together: FI plus the neighbours the model lags on. */
const SCORED_AREAS: readonly string[] = ["FI", "SE1", "SE3", "EE"];

// ---------------------------------------------------------------------------
// Parity-critical pure assembly (no I/O)
// ---------------------------------------------------------------------------

/**
 * Assemble `BacktestData` from DB rows, SHAPE-IDENTICAL to what `loadFixture`
 * produces from the equivalent fixture JSON — this is the parity-critical
 * surface, so it is pure and unit-tested against `loadFixture`.
 *
 * Prices become `PricePoint`s with the EUR/MWh → c/kWh conversion applied
 * EXACTLY ONCE via `eurMwhToCentsKwh` (which rounds to 3 dp). Actuals (75/124)
 * and forecast VINTAGES (245/165) are passed through RAW (no pre-bucketing —
 * `buildForecast` handles the hourly→15-min expansion for dataset 124), keyed by
 * STRING dataset ids.
 */
export const assembleBacktestData = (
  pricesByArea: ReadonlyMap<string, readonly HourlyPrice[]>,
  fingridActualsByDataset: Readonly<Record<string, readonly FingridRecord[]>>,
  fingridForecastVintagesByDataset: Readonly<
    Record<string, readonly ForecastVintageRecord[]>
  >,
): BacktestData => {
  const prices: PricePoint[] = [];
  for (const [area, rows] of pricesByArea) {
    for (const row of rows) {
      prices.push({
        area,
        start: row.deliveryStart,
        spotCentsKwh: eurMwhToCentsKwh(row.priceEurMwh),
      });
    }
  }
  return {
    prices,
    fingridActualsByDataset,
    fingridForecastVintagesByDataset,
  };
};

// ---------------------------------------------------------------------------
// DB fetch (I/O — the only side-effecting surface besides the script entry)
// ---------------------------------------------------------------------------

/**
 * Fetch the scoring window `[now − windowDays, now]` from the DB: FI together
 * with the neighbour areas in ONE `getPricesByAreas` call, and each Fingrid
 * dataset by range. Returns assembled `BacktestData` (via the pure helper).
 */
export const fetchBacktestData = async (
  pool: pg.Pool,
  windowDays: number,
  now: Date = new Date(),
): Promise<BacktestData> => {
  const startUtc = new Date(now.getTime() - windowDays * DAY_MS).toISOString();
  const endUtc = now.toISOString();

  const pricesByArea = await getPricesByAreas(
    pool,
    startUtc,
    endUtc,
    SCORED_AREAS,
  );

  // Actuals (75/124) from the single-valued actuals table.
  const fingridActualsByDataset: Record<string, FingridRecord[]> = {};
  for (const id of [DATASET_WIND_ACTUAL, DATASET_CONSUMPTION_ACTUAL]) {
    const records = await getFingridRecordsByRange(pool, id, startUtc, endUtc);
    fingridActualsByDataset[String(id)] = [...records];
  }

  // Forecast datasets (245/165) as the full per-issuance vintage ladder — the
  // read #80 shares with the revision study (#79), NOT the emptied actuals
  // table (migration 006 deleted 245/165 there).
  const fingridForecastVintagesByDataset: Record<
    string,
    ForecastVintageRecord[]
  > = {};
  for (const id of [DATASET_WIND_FORECAST, DATASET_CONSUMPTION_FORECAST]) {
    const records = await getFingridForecastVintagesAll(
      pool,
      id,
      startUtc,
      endUtc,
    );
    fingridForecastVintagesByDataset[String(id)] = [...records];
  }

  return assembleBacktestData(
    pricesByArea,
    fingridActualsByDataset,
    fingridForecastVintagesByDataset,
  );
};

// ---------------------------------------------------------------------------
// Export (snapshot DB → loadFixture-compatible fixture.json)
// ---------------------------------------------------------------------------

/**
 * Serialise assembled `BacktestData` as a `loadFixture`-compatible fixture. The
 * `spotCentsKwh` values are ALREADY post-conversion, so a later `--data` replay
 * reads them as-is and does NOT convert again.
 */
export const toFixtureJson = (data: BacktestData): string => {
  const fingridActuals: Record<
    string,
    { startTime: string; endTime: string; value: number }[]
  > = {};
  for (const [dataset, records] of Object.entries(
    data.fingridActualsByDataset,
  )) {
    fingridActuals[dataset] = records.map((r) => ({
      startTime: r.startTime,
      endTime: r.endTime,
      value: r.value,
    }));
  }
  const fingridForecastVintages: Record<
    string,
    { issuedAt: string; startTime: string; endTime: string; value: number }[]
  > = {};
  for (const [dataset, records] of Object.entries(
    data.fingridForecastVintagesByDataset,
  )) {
    fingridForecastVintages[dataset] = records.map((r) => ({
      issuedAt: r.issuedAt,
      startTime: r.startTime,
      endTime: r.endTime,
      value: r.value,
    }));
  }
  const fixture = {
    prices: data.prices.map((p) => ({
      area: p.area,
      start: p.start,
      spotCentsKwh: p.spotCentsKwh,
    })),
    fingridActuals,
    fingridForecastVintages,
  };
  return JSON.stringify(fixture, null, 2);
};

// ---------------------------------------------------------------------------
// Thin-data guard (pure decision)
// ---------------------------------------------------------------------------

/**
 * Whether the run has too few scoreable origins for the metrics to be a verdict.
 * Pure so it can be unit-tested without a DB.
 */
export const isThinData = (origins: number): boolean =>
  origins < MIN_SCOREABLE_ORIGINS;

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

const fmt = (v: number | null, dec = 3): string =>
  v !== null ? v.toFixed(dec) : "n/a";

const sub = (a: number | null, b: number | null): number | null =>
  a === null || b === null ? null : a - b;

const printReport = (data: BacktestData): boolean => {
  const summary = runBacktest(data);
  const { offsetsByHour, globalOffsets } = deriveBandOffsets(summary.residuals);
  const coverage = observedCoverageOf(
    summary.residuals,
    offsetsByHour,
    globalOffsets,
  );

  const thin = isThinData(summary.origins);
  if (thin) {
    console.warn(
      `WARNING: only ${String(summary.origins)} scoreable origins (need ${String(
        MIN_SCOREABLE_ORIGINS,
      )}) — metrics are not yet a verdict.`,
    );
  }

  console.log(`\nFI forecast backtest (issue-time keyed)`);
  console.log(`  origins scored:      ${String(summary.origins)}`);
  console.log(`  fallback origins:    ${String(summary.fallbackOrigins)}`);
  console.log(`  leakage-free:        ${String(summary.leakFree)}`);
  console.log(`  model MAE  (c/kWh):  ${fmt(summary.modelMae)}`);
  console.log(`  model sMAPE:         ${fmt(summary.modelSmape)}`);
  console.log(`  rMAE vs last_week:           ${fmt(summary.rMae.last_week)}`);
  console.log(
    `  rMAE vs last_published_day:  ${fmt(summary.rMae.last_published_day)}`,
  );
  console.log(
    `  band residuals:      ${String(summary.residuals.length)}  hour-buckets ${String(
      offsetsByHour.size,
    )}  observed coverage ${fmt(coverage)}`,
  );

  // Per-horizon: absolute (MAE/sMAPE), rank (Spearman, precision@N for the
  // cheapest/peak N-quarter window — the actual flexible-load use case), and
  // band coverage. d+1 is the first forecast day (the day after the last
  // published price).
  const nHours = PRECISION_N_QUARTERS / 4;
  console.log(
    `\n  per horizon (precision@N over the cheapest/peak ${String(
      PRECISION_N_QUARTERS,
    )} quarters = ${String(nHours)}h):`,
  );
  console.log(
    `    horizon  pairs     MAE   sMAPE   Spearman  P@N-cheap  P@N-peak  bandCov`,
  );
  for (const h of HORIZON_LABELS) {
    const m = summary.byHorizon[h];
    console.log(
      `    ${h.padEnd(7)}  ${String(m.pairs).padStart(5)}  ${fmt(m.mae).padStart(6)}  ${fmt(
        m.smape,
      ).padStart(6)}  ${fmt(m.spearman).padStart(8)}  ${fmt(
        m.precisionAtNCheap,
      ).padStart(9)}  ${fmt(m.precisionAtNPeak).padStart(8)}  ${fmt(
        m.bandCoverage,
      ).padStart(7)}`,
    );
  }
  console.log("");

  return !thin;
};

// ---------------------------------------------------------------------------
// Optimism comparison report (--compare) — the #80 deliverable
// ---------------------------------------------------------------------------

/** Sparse-ladder fraction above which the delta is likely understated. */
const SPARSE_WARN = 0.2;

const printCompare = (data: BacktestData): boolean => {
  const cmp: OptimismComparison = compareOptimism(data);
  const { leaked, honest } = cmp;

  const thin = isThinData(cmp.scoredOrigins);
  if (thin) {
    console.warn(
      `WARNING: only ${String(cmp.scoredOrigins)} shared origins (need ${String(
        MIN_SCOREABLE_ORIGINS,
      )}) — the delta is not yet a verdict.`,
    );
  }

  console.log(`\nVintage-leak optimism (leaked latest vs honest issue-time)`);
  console.log(
    `  shared origins:      ${String(cmp.scoredOrigins)}  (pre-vintage skipped: ${String(
      honest.preVintageOrigins,
    )})`,
  );
  console.log(
    `  scored window:       ${honest.scoredWindowStart ?? "n/a"} … ${
      honest.scoredWindowEnd ?? "n/a"
    }  (${honest.scoredWindowSpanDays.toFixed(1)} days)`,
  );
  console.log(
    `  honest leak-free:    ${String(honest.leakFree)}   leaked leak-free: ${String(
      leaked.leakFree,
    )}  (leaked is FALSE BY DESIGN — the vintage postdates issue time)`,
  );

  // FIXED caveat block (da cut 2) — printed verbatim every run.
  console.log(`\n  CAVEATS (read before quoting any number):`);
  console.log(
    `   - SIGN: delta = leaked − honest (positive ⇒ leaked was optimistic).`,
  );
  console.log(
    `   - N=${String(cmp.scoredOrigins)} DAILY origins, strongly autocorrelated → treat as DIRECTIONAL, not a precise interval (no CI is reported).`,
  );
  console.log(
    `   - SUMMER WIND-ONLY LOWER BOUND on the annual leak: 165 shows no summer revision (#79), so winter would only widen it.`,
  );
  console.log(
    `   - delta ≈ 0 ALONE MUST NOT close #81 — re-run with autumn/winter accumulation first.`,
  );

  console.log(`\n  metric            leaked      honest       delta`);
  const row = (label: string, l: number | null, h: number | null): void => {
    console.log(
      `  ${label.padEnd(16)}${fmt(l).padStart(8)}    ${fmt(h).padStart(8)}    ${fmt(
        sub(l, h),
      ).padStart(8)}`,
    );
  };
  row("MAE (c/kWh)", leaked.modelMae, honest.modelMae);
  row("sMAPE", leaked.modelSmape, honest.modelSmape);
  row("rMAE last_week", leaked.rMae.last_week, honest.rMae.last_week);
  row(
    "rMAE last_pub_day",
    leaked.rMae.last_published_day,
    honest.rMae.last_published_day,
  );
  console.log(
    `  band coverage     ${"—".padStart(8)}    ${"—".padStart(8)}    ${fmt(
      cmp.deltaBandCoverage,
    ).padStart(8)}`,
  );

  console.log(`\n  per horizon (delta = leaked − honest):`);
  console.log(`    horizon   deltaMAE   deltaSMAPE   deltaBandCov`);
  for (const h of HORIZON_LABELS) {
    const d = cmp.deltaByHorizon[h];
    console.log(
      `    ${h.padEnd(7)}   ${fmt(d.deltaMae).padStart(8)}   ${fmt(
        d.deltaSmape,
      ).padStart(10)}   ${fmt(d.deltaBandCoverage).padStart(12)}`,
    );
  }

  console.log(`\n  ladder depth (cron-gap diagnostic):`);
  console.log(`    dataset   medianDepth   expectedDepth(p90)   sparseFrac`);
  for (const [id, ld] of Object.entries(cmp.ladderDiagnostic)) {
    console.log(
      `    ${id.padEnd(7)}   ${fmt(ld.medianDepth, 1).padStart(11)}   ${fmt(
        ld.expectedDepth,
        1,
      ).padStart(18)}   ${fmt(ld.sparseTargetFraction, 3).padStart(10)}`,
    );
    if (
      ld.sparseTargetFraction !== null &&
      ld.sparseTargetFraction > SPARSE_WARN
    ) {
      console.warn(
        `    WARNING: dataset ${id} has ${fmt(
          ld.sparseTargetFraction,
          3,
        )} thin ladders — cron gaps may UNDERSTATE the delta.`,
      );
    }
  }
  console.log("");

  return !thin;
};

// ---------------------------------------------------------------------------
// Flag parsing (hand-rolled — no cli-arg dependency)
// ---------------------------------------------------------------------------

const flagValue = (
  argv: readonly string[],
  flag: string,
): string | undefined => {
  const idx = argv.indexOf(flag);
  if (idx < 0) {
    return undefined;
  }
  return argv[idx + 1];
};

const hasFlag = (argv: readonly string[], flag: string): boolean =>
  argv.includes(flag);

/** Parse `--window <days>`: a positive integer, default `DEFAULT_WINDOW_DAYS`. */
export const parseWindowDays = (raw: string | undefined): number => {
  if (raw === undefined) {
    return DEFAULT_WINDOW_DAYS;
  }
  const value = Number(raw);
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(
      `--window must be a positive integer number of days, got "${raw}"`,
    );
  }
  return value;
};

// ---------------------------------------------------------------------------
// Script entry (dev only)
// ---------------------------------------------------------------------------

const main = async (): Promise<void> => {
  const argv = process.argv;

  // Fixture replay: pure, no DB. `--data` takes a fixture FILE path and
  // `loadFixture` reads post-conversion values (symmetric with `--export`).
  const dataFile = flagValue(argv, "--data");
  if (dataFile !== undefined) {
    const data = loadFixture(dataFile);
    const ok = printReport(data);
    process.exit(ok ? 0 : 1);
  }

  if (!hasFlag(argv, "--db")) {
    console.error(
      "usage: pnpm backtest --data <fixture.json> | --db [--window <days>] [--export <fixture.json>] [--compare]",
    );
    process.exit(2);
  }

  const windowDays = parseWindowDays(flagValue(argv, "--window"));
  const env = loadEnv();
  const connectionString = env.DATABASE_PUBLIC_URL ?? env.DATABASE_URL;
  if (!connectionString) {
    throw new Error(
      "DATABASE_PUBLIC_URL or DATABASE_URL must be set for `pnpm backtest --db`.",
    );
  }

  // Plain read pool — do NOT use initDatabase (it runs migrations).
  const pool = new Pool({ connectionString });
  try {
    const data = await fetchBacktestData(pool, windowDays);

    const exportFile = flagValue(argv, "--export");
    if (exportFile !== undefined) {
      writeFileSync(exportFile, toFixtureJson(data), "utf-8");
      console.log(
        `Exported ${String(data.prices.length)} prices → ${exportFile}`,
      );
    }

    // --compare measures the vintage-leak optimism (issue #80): honest
    // issue-time vs leaked latest-vintage over the same origins.
    const ok = hasFlag(argv, "--compare")
      ? printCompare(data)
      : printReport(data);
    process.exit(ok ? 0 : 1);
  } finally {
    await closeDatabase(pool);
  }
};

// Run only when invoked as a script, never on import.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  void main();
}

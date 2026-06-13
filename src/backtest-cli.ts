/**
 * OFFLINE / DEV-ONLY backtest CLI for the FI forecast.
 *
 * IMPORTANT: this module must NOT be imported by any route, `index.ts`,
 * `app.ts`, or `scheduler.ts`, and is NOT part of `$BUILD_CMD` or the request
 * path. Like `backtest.ts` and `regenerate-bands.ts`, the production bundle's
 * only tsup entry is `src/index.ts`; nothing in that runtime graph imports this
 * file, so it never reaches `dist/`. It runs only as a tsx script:
 *
 *     pnpm backtest --data tools/backtest-data     # replay a committed fixture
 *     pnpm backtest --db                           # score against the live DB
 *     pnpm backtest --db --window 120              # widen the DB window (days)
 *     pnpm backtest --db --export tools/x/fixture.json  # snapshot DB → fixture
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
import { closeDatabase } from "./db.js";
import pg from "pg";
import { loadEnv } from "./env.js";
import {
  loadFixture,
  runBacktest,
  type BacktestData,
  type PricePoint,
} from "./backtest.js";
import { deriveBandOffsets, observedCoverageOf } from "./conformal.js";
import { getPricesByAreas } from "./price-store.js";
import { getFingridRecordsByRange } from "./fingrid-store.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "./fingrid.js";
import { eurMwhToCentsKwh } from "./nordpool.js";
import type { FingridRecord, HourlyPrice } from "./types.js";

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
 * EXACTLY ONCE via `eurMwhToCentsKwh` (which rounds to 3 dp). Fingrid records
 * are passed through RAW (no pre-bucketing — `buildForecast` handles the
 * hourly→15-min expansion for dataset 124), keyed by STRING dataset ids.
 */
export const assembleBacktestData = (
  pricesByArea: ReadonlyMap<string, readonly HourlyPrice[]>,
  fingridByDataset: Readonly<Record<string, readonly FingridRecord[]>>,
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
  return { prices, fingridByDataset };
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

  const datasetIds = [
    DATASET_WIND_FORECAST,
    DATASET_WIND_ACTUAL,
    DATASET_CONSUMPTION_FORECAST,
    DATASET_CONSUMPTION_ACTUAL,
  ];
  const fingridByDataset: Record<string, FingridRecord[]> = {};
  for (const id of datasetIds) {
    const records = await getFingridRecordsByRange(pool, id, startUtc, endUtc);
    fingridByDataset[String(id)] = [...records];
  }

  return assembleBacktestData(pricesByArea, fingridByDataset);
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
  const fingrid: Record<
    string,
    { startTime: string; endTime: string; value: number }[]
  > = {};
  for (const [dataset, records] of Object.entries(data.fingridByDataset)) {
    fingrid[dataset] = records.map((r) => ({
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
    fingrid,
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

  // Fixture replay: pure, no DB. `loadFixture` reads post-conversion values.
  const dataDir = flagValue(argv, "--data");
  if (dataDir !== undefined) {
    const data = loadFixture(dataDir);
    const ok = printReport(data);
    process.exit(ok ? 0 : 1);
  }

  if (!hasFlag(argv, "--db")) {
    console.error(
      "usage: pnpm backtest --data <dir> | --db [--window <days>] [--export <file>]",
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

    // --db replays whatever Fingrid FORECAST rows are currently stored, which
    // have been overwritten since their issue time, so rMAE is slightly
    // optimistic. Fully-honest coverage needs live-forecast logging (deferred).
    console.log(
      "note: --db replays current Fingrid forecasts (overwritten since issue time) → rMAE is slightly optimistic; fully-honest coverage needs live-forecast logging (deferred).",
    );

    const ok = printReport(data);
    process.exit(ok ? 0 : 1);
  } finally {
    await closeDatabase(pool);
  }
};

// Run only when invoked as a script, never on import.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  void main();
}

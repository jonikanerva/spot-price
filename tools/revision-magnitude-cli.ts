/**
 * OFFLINE / DEV-ONLY CLI for the Fingrid forecast-revision study (issue #79).
 *
 * IMPORTANT: this module lives in `tools/` (offline-only). It must NOT be
 * imported by any `src/` runtime module — the production bundle's only tsup
 * entry is `src/index.ts`, and an ESLint guard forbids `src/` runtime from
 * importing `tools/`, so this file never reaches `dist/`. It runs only as a tsx
 * script:
 *
 *     pnpm revision-magnitude --data tools/backtest-data/vintages.json  # replay
 *     pnpm revision-magnitude --db                          # study the live DB
 *     pnpm revision-magnitude --db --window 60              # widen the window
 *     pnpm revision-magnitude --db --export tools/x/v.json  # snapshot DB → file
 *
 * It reconstructs, per forecast dataset and lead-time bin, how much the archived
 * vintages (`fingrid_forecasts`, issue #78) were revised between early issuance
 * and delivery, and prints a GO / MARGINAL / DEFER recommendation for #81. All
 * DB I/O lives here; `revision-magnitude.ts` stays pure. This is NOT a scheduled
 * job and adds no endpoint (`STACK §9`). It never logs the connection string or
 * any secret (`STACK §8`).
 *
 * The all-vintages ladder read lives in `src/fingrid-store.ts`
 * (`getFingridForecastVintagesAll`), NOT inline here: #80's vintage-correct
 * backtest needs the same read, `backtest-cli.ts` already imports store reads
 * from `src/`, and one home avoids duplicating the SQL + row mapping (architect
 * final design — reversing the earlier CLI-local plan). A pure read the server
 * never calls is not a server behaviour change.
 */
// Import db.js FIRST for its TIMESTAMPTZ→ISO type-parser side-effect (db.ts):
// without it `pg` returns TIMESTAMPTZ columns as `Date` objects, which would
// break the ms-arithmetic in the engine and the fixture-parity round-trip.
import { closeDatabase } from "../src/db.js";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import pg from "pg";
import { loadEnv } from "../src/env.js";
import { parseWindowDays } from "./backtest-cli.js";
import {
  recommendation,
  REFERENCE_MAX_LEAD_H,
  runRevisionStudy,
  VINTAGE_DATASET_IDS,
  type ActualRecord,
  type DatasetRevisionSummary,
  type RevisionStudyInput,
} from "./revision-magnitude.js";
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
import type { ForecastVintageRecord } from "../src/types.js";

const { Pool } = pg;

const DAY_MS = 24 * 60 * 60 * 1000;

/** Actual dataset paired with each forecast dataset, for the sanity check. */
const ACTUAL_FOR_FORECAST: Readonly<Record<number, number>> = {
  [DATASET_WIND_FORECAST]: DATASET_WIND_ACTUAL,
  [DATASET_CONSUMPTION_FORECAST]: DATASET_CONSUMPTION_ACTUAL,
};

/** Exclusion rate above which the report warns that the reference is unreliable. */
const EXCLUSION_WARN = 0.25;

// ---------------------------------------------------------------------------
// Assembly + fixture round-trip (pure, parity-critical)
// ---------------------------------------------------------------------------

/** Assemble a study input from vintages + actuals keyed by string dataset id. */
export const assembleStudyInput = (
  vintagesByDataset: Readonly<Record<string, readonly ForecastVintageRecord[]>>,
  actualsByDataset: Readonly<Record<string, readonly ActualRecord[]>>,
): RevisionStudyInput => ({ vintagesByDataset, actualsByDataset });

interface VintageFixture {
  readonly vintages: Readonly<Record<string, readonly ForecastVintageRecord[]>>;
  readonly actuals?: Readonly<Record<string, readonly ActualRecord[]>>;
}

/** Serialise a study input as a `loadVintageFixture`-compatible JSON string. */
export const toFixtureJson = (input: RevisionStudyInput): string => {
  const vintages: Record<string, ForecastVintageRecord[]> = {};
  for (const [dataset, records] of Object.entries(input.vintagesByDataset)) {
    vintages[dataset] = records.map((r) => ({
      datasetId: r.datasetId,
      issuedAt: r.issuedAt,
      startTime: r.startTime,
      endTime: r.endTime,
      value: r.value,
    }));
  }
  const actuals: Record<string, ActualRecord[]> = {};
  for (const [dataset, records] of Object.entries(
    input.actualsByDataset ?? {},
  )) {
    actuals[dataset] = records.map((r) => ({
      datasetId: r.datasetId,
      startTime: r.startTime,
      value: r.value,
    }));
  }
  return JSON.stringify({ vintages, actuals }, null, 2);
};

/** Read a study input from a fixture FILE path (symmetric with `--export`). */
export const loadVintageFixture = (filePath: string): RevisionStudyInput => {
  const text = readFileSync(filePath, "utf-8");
  const fixture = JSON.parse(text) as VintageFixture;
  const vintagesByDataset: Record<string, ForecastVintageRecord[]> = {};
  for (const [dataset, records] of Object.entries(fixture.vintages)) {
    vintagesByDataset[dataset] = records.map((r) => ({
      datasetId: r.datasetId,
      issuedAt: r.issuedAt,
      startTime: r.startTime,
      endTime: r.endTime,
      value: r.value,
    }));
  }
  const actualsByDataset: Record<string, ActualRecord[]> = {};
  for (const [dataset, records] of Object.entries(fixture.actuals ?? {})) {
    actualsByDataset[dataset] = records.map((r) => ({
      datasetId: r.datasetId,
      startTime: r.startTime,
      value: r.value,
    }));
  }
  return assembleStudyInput(vintagesByDataset, actualsByDataset);
};

// ---------------------------------------------------------------------------
// DB fetch (I/O — the only side-effecting surface besides the script entry)
// ---------------------------------------------------------------------------

/**
 * Fetch every archived vintage for the forecast datasets over
 * `[now − windowDays, now]` (the full lead-time ladder, via the store read),
 * plus the paired actuals for the secondary sanity check.
 */
export const fetchStudyInput = async (
  pool: pg.Pool,
  windowDays: number,
  now: Date = new Date(),
): Promise<RevisionStudyInput> => {
  const startUtc = new Date(now.getTime() - windowDays * DAY_MS).toISOString();
  const endUtc = now.toISOString();

  const vintagesByDataset: Record<string, ForecastVintageRecord[]> = {};
  for (const id of VINTAGE_DATASET_IDS) {
    const records = await getFingridForecastVintagesAll(
      pool,
      id,
      startUtc,
      endUtc,
    );
    vintagesByDataset[String(id)] = [...records];
  }

  const actualsByDataset: Record<string, ActualRecord[]> = {};
  for (const id of VINTAGE_DATASET_IDS) {
    const actualId = ACTUAL_FOR_FORECAST[id];
    if (actualId === undefined) {
      continue;
    }
    const records = await getFingridRecordsByRange(
      pool,
      actualId,
      startUtc,
      endUtc,
    );
    actualsByDataset[String(actualId)] = records.map((r) => ({
      datasetId: r.datasetId,
      startTime: r.startTime,
      value: r.value,
    }));
  }

  return assembleStudyInput(vintagesByDataset, actualsByDataset);
};

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

const fmt = (v: number | null, dec = 2): string =>
  v !== null ? v.toFixed(dec) : "n/a";

const day = (iso: string): string => iso.slice(0, 10);

const printDataset = (d: DatasetRevisionSummary): void => {
  console.log(`\ndataset ${String(d.datasetId)} (forecast):`);
  console.log(
    `  targets:   admissible ${String(d.admissibleTargets)}  excluded ${String(
      d.excludedTargets,
    )}  future ${String(d.futureTargets)}  exclusion-rate ${fmt(d.exclusionRate, 3)}`,
  );
  if (d.exclusionRate !== null && d.exclusionRate > EXCLUSION_WARN) {
    console.warn(
      `  WARNING: exclusion rate ${fmt(d.exclusionRate, 3)} > ${fmt(
        EXCLUSION_WARN,
        2,
      )} — many targets lack a near-delivery reference (job gaps?); treat the reference as less reliable.`,
    );
  }
  console.log(
    `  reference: post-delivery ${String(
      d.referencesPostDelivery,
    )}  pre-delivery≤${String(REFERENCE_MAX_LEAD_H)}h ${String(
      d.referencesPreDeliveryWithinTol,
    )}`,
  );
  console.log(
    `  empirical lead: max ${fmt(d.empiricalMaxLeadH, 1)}h  p90 ${fmt(
      d.empiricalP90LeadH,
      1,
    )}h   (gate on this band — the served forecast has NO Fingrid feature beyond it)`,
  );
  console.log(
    `  reference series: sd ${fmt(d.sdReference, 1)} MW  mean|ref| ${fmt(
      d.meanAbsReference,
      1,
    )} MW`,
  );
  if (d.actualCheck !== null) {
    console.log(
      `  ref-vs-actual sanity: ${String(
        d.actualCheck.targetsCompared,
      )} targets  median|ref−actual| ${fmt(
        d.actualCheck.medianAbsRefMinusActual,
        1,
      )} MW  mean ${fmt(d.actualCheck.meanAbsRefMinusActual, 1)} MW`,
    );
  } else {
    console.log(`  ref-vs-actual sanity: no overlapping actuals (skipped)`);
  }
  console.log(
    `  aggregate (sufficient bins): rms ${fmt(d.productionBandRms, 1)} MW  NSR ${fmt(
      d.productionBandNsr,
      3,
    )}  attenuation(illustrative) ${fmt(d.attenuationIllustration, 3)}  samples ${String(
      d.productionBandSamples,
    )}`,
  );
  console.log(
    `    bin       samples  targets  meanAbsΔ  medAbsΔ  p90AbsΔ    rmsΔ   biasΔ    NSR   atten  relMeanAbs`,
  );
  for (const b of d.buckets) {
    if (!b.sufficient) {
      // Thin bin: show the count, suppress the stats (da / final design).
      const tag = b.samples > 0 ? "insufficient" : "empty";
      console.log(
        `    ${b.label.padEnd(8)}  ${String(b.samples).padStart(7)}  ${String(
          b.targets,
        ).padStart(7)}  ${tag}`,
      );
      continue;
    }
    console.log(
      `    ${b.label.padEnd(8)}  ${String(b.samples).padStart(7)}  ${String(
        b.targets,
      ).padStart(7)}  ${fmt(b.meanAbsRevision, 1).padStart(8)}  ${fmt(
        b.medianAbsRevision,
        1,
      ).padStart(7)}  ${fmt(b.p90AbsRevision, 1).padStart(7)}  ${fmt(
        b.rmsRevision,
        1,
      ).padStart(6)}  ${fmt(b.signedBiasRevision, 1).padStart(6)}  ${fmt(
        b.noiseToSignal,
        3,
      ).padStart(5)}  ${fmt(b.attenuation, 3).padStart(6)}  ${fmt(
        b.relMeanAbs,
        3,
      ).padStart(8)}`,
    );
  }
};

/**
 * Print the full report. Returns false (→ non-zero exit) when the verdict is
 * DEFER, so a wrapper cannot read thin data as a decision.
 */
const printReport = (input: RevisionStudyInput): boolean => {
  const result = runRevisionStudy(input);
  const rec = recommendation(result);

  console.log(`\nFingrid forecast-revision study (issue #79)`);
  if (result.window !== null) {
    console.log(
      `  window: targets ${day(result.window.earliestTarget)} … ${day(
        result.window.latestTarget,
      )}  (issued ${day(result.window.earliestIssuedAt)} … ${day(
        result.window.latestIssuedAt,
      )})`,
    );
    console.log(
      `  CAVEAT (summer-only): this window covers a single-season regime; wind/consumption revision dynamics differ in winter, so a decision to CLOSE #81 cannot be recorded from this window alone — only GO / MARGINAL / DEFER (da amendment 1).`,
    );
  } else {
    console.log(`  window: no vintages found`);
  }
  console.log(
    `  CAVEAT (lead resolution): issued_at is an hourly FETCH-TIME proxy (±~1h, migration 005), so bin edges and the ${String(
      REFERENCE_MAX_LEAD_H,
    )}h reference cutoff inherit ±1h uncertainty.`,
  );
  console.log(
    `  Δ = revision = value@lead − reference (freshest issuance per target); bins are lead-time (loH, hiH] in hours.`,
  );

  for (const d of result.datasets) {
    printDataset(d);
  }

  console.log(`\nrecommendation: ${rec.verdict}`);
  console.log(`  ${rec.reason}`);
  console.log("");

  if (rec.verdict === "DEFER") {
    console.warn(
      "DEFER — insufficient data for a verdict; exiting non-zero so this is not read as a decision.",
    );
    return false;
  }
  return true;
};

// ---------------------------------------------------------------------------
// Flag parsing (hand-rolled — reuses the backtest CLI's window parser)
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

// ---------------------------------------------------------------------------
// Script entry (dev only)
// ---------------------------------------------------------------------------

const main = async (): Promise<void> => {
  const argv = process.argv;

  const dataFile = flagValue(argv, "--data");
  if (dataFile !== undefined) {
    const input = loadVintageFixture(dataFile);
    const ok = printReport(input);
    process.exit(ok ? 0 : 1);
  }

  if (!hasFlag(argv, "--db")) {
    console.error(
      "usage: pnpm revision-magnitude --data <fixture.json> | --db [--window <days>] [--export <fixture.json>]",
    );
    process.exit(2);
  }

  const windowDays = parseWindowDays(flagValue(argv, "--window"));
  const env = loadEnv();
  const connectionString = env.DATABASE_PUBLIC_URL ?? env.DATABASE_URL;
  if (!connectionString) {
    throw new Error(
      "DATABASE_PUBLIC_URL or DATABASE_URL must be set for `pnpm revision-magnitude --db`.",
    );
  }

  // Plain read pool — do NOT use initDatabase (it runs migrations).
  const pool = new Pool({ connectionString });
  try {
    const input = await fetchStudyInput(pool, windowDays);

    const exportFile = flagValue(argv, "--export");
    if (exportFile !== undefined) {
      writeFileSync(exportFile, toFixtureJson(input), "utf-8");
      const vintageCount = Object.values(input.vintagesByDataset).reduce(
        (sum, rows) => sum + rows.length,
        0,
      );
      console.log(`Exported ${String(vintageCount)} vintages → ${exportFile}`);
    }

    const ok = printReport(input);
    process.exit(ok ? 0 : 1);
  } finally {
    await closeDatabase(pool);
  }
};

// Run only when invoked as a script, never on import.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  void main();
}

/**
 * Rolling-origin backtest for the FI price forecast.
 *
 * Establishes a measured baseline for forecast quality. The forecast's job is
 * automation (picking cheap/expensive hours), so the report leads with rank
 * metrics (Spearman, precision@N for cheapest/peak hours) and treats MAE/RMSE/
 * bias as reference. Accuracy is reported per horizon (day 1/2/3) and against
 * naive baselines the model must beat to justify itself (last_week, yesterday,
 * flat) — so `degraded`/accuracy means something honest.
 *
 * No network, no API keys, no DB: reads JSON fixtures from tools/backtest-data/
 * and imports the production `buildForecast` (never modified). Pure stdlib.
 *
 * Run with: pnpm tsx tools/backtest.ts [--data tools/backtest-data]
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { buildForecast, quarterKey } from "../src/forecast.js";
import type { FingridRecord } from "../src/types.js";
import { bias, mae, precisionAtN, rmse, spearman } from "./backtest-metrics.js";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const FORECAST_DAYS = 3;
const PUBLICATION_HOUR_UTC = 12; // FI day-ahead ~14:00 EET ≈ 12:00 UTC

const HORIZONS = ["day1", "day2", "day3"] as const;
type Horizon = (typeof HORIZONS)[number];

interface RawPrice {
  readonly start: string;
  readonly spotCentsKwh: number;
}
interface RawFingrid {
  readonly startTime: string;
  readonly endTime: string;
  readonly value: number;
}
interface Fixture {
  readonly prices: readonly RawPrice[];
  readonly fingrid: Readonly<Record<string, readonly RawFingrid[]>>;
}

const toRecords = (
  raw: readonly RawFingrid[],
  datasetId: number,
): FingridRecord[] =>
  raw.map((r) => ({
    datasetId,
    startTime: r.startTime,
    endTime: r.endTime,
    value: r.value,
  }));

const ms = (iso: string): number => new Date(iso).getTime();

const loadFixture = (dir: string): Fixture => {
  const text = readFileSync(path.join(dir, "fixture.json"), "utf-8");
  return JSON.parse(text) as Fixture;
};

/** Realized spot price by quarter key — uncensored ground truth for scoring. */
const buildRealized = (
  prices: readonly RawPrice[],
): ReadonlyMap<string, number> => {
  const out = new Map<string, number>();
  for (const p of prices) {
    out.set(quarterKey(ms(p.start)), p.spotCentsKwh);
  }
  return out;
};

/** Prices known at origin T: delivered before the publication horizon. */
const censorPrices = (
  prices: readonly RawPrice[],
  tMs: number,
): Map<string, number> => {
  const tDayStart = Math.floor(tMs / DAY_MS) * DAY_MS;
  const todayEnd = tDayStart + DAY_MS;
  const tHour = new Date(tMs).getUTCHours();
  const horizonEnd =
    tHour >= PUBLICATION_HOUR_UTC ? todayEnd + DAY_MS : todayEnd;
  const out = new Map<string, number>();
  for (const p of prices) {
    if (ms(p.start) < horizonEnd) {
      out.set(quarterKey(ms(p.start)), p.spotCentsKwh);
    }
  }
  return out;
};

/** Fingrid forecast quarters knowable at T: within T + horizon hours. */
const censorForecast = (
  raw: readonly RawFingrid[],
  tMs: number,
  horizonHours: number,
): RawFingrid[] => {
  const cutoff = tMs + horizonHours * HOUR_MS;
  return raw.filter((r) => ms(r.startTime) < cutoff);
};

/** Actual observations strictly before T. */
const censorActuals = (raw: readonly RawFingrid[], tMs: number): RawFingrid[] =>
  raw.filter((r) => ms(r.startTime) < tMs);

const lastKnownPriceMs = (
  known: ReadonlyMap<string, number>,
): number | null => {
  let max = -Infinity;
  for (const key of known.keys()) {
    const t = ms(key);
    if (t > max) {
      max = t;
    }
  }
  return max === -Infinity ? null : max;
};

const horizonOf = (dtMs: number, seriesStartMs: number): Horizon | null => {
  const hours = (dtMs - seriesStartMs) / HOUR_MS;
  if (hours >= 0 && hours < 24) return "day1";
  if (hours >= 24 && hours < 48) return "day2";
  if (hours >= 48 && hours < 72) return "day3";
  return null;
};

/** Aggregate 15-min (pred, act) triples to hourly means. */
const toHourly = (
  triples: readonly (readonly [number, number, number])[],
): { preds: number[]; acts: number[] } => {
  const byHour = new Map<number, [number, number][]>();
  for (const [dtMs, p, a] of triples) {
    const hk = Math.floor(dtMs / HOUR_MS) * HOUR_MS;
    const bucket = byHour.get(hk) ?? [];
    bucket.push([p, a]);
    byHour.set(hk, bucket);
  }
  const hours = [...byHour.keys()].sort((x, y) => x - y);
  const preds: number[] = [];
  const acts: number[] = [];
  for (const h of hours) {
    const bucket = byHour.get(h) ?? [];
    preds.push(bucket.reduce((s, x) => s + x[0], 0) / bucket.length);
    acts.push(bucket.reduce((s, x) => s + x[1], 0) / bucket.length);
  }
  return { preds, acts };
};

interface Accumulator {
  pairs: [number, number][];
  spearman: number[];
  pCheap: number[];
  pPeak: number[];
  nOrigins: number;
}
const newAcc = (): Accumulator => ({
  pairs: [],
  spearman: [],
  pCheap: [],
  pPeak: [],
  nOrigins: 0,
});

const accumulate = (
  acc: Accumulator,
  triples: readonly (readonly [number, number, number])[],
  cheapN: number,
  peakN: number,
): void => {
  if (triples.length === 0) {
    return;
  }
  for (const [, p, a] of triples) {
    acc.pairs.push([p, a]);
  }
  const { preds, acts } = toHourly(triples);
  if (preds.length < 2) {
    return;
  }
  acc.nOrigins++;
  const sp = spearman(preds, acts);
  if (sp !== null) acc.spearman.push(sp);
  const pc = precisionAtN(preds, acts, cheapN, "cheap");
  if (pc !== null) acc.pCheap.push(pc);
  const pp = precisionAtN(preds, acts, peakN, "peak");
  if (pp !== null) acc.pPeak.push(pp);
};

const mean = (xs: readonly number[]): number | null =>
  xs.length > 0 ? xs.reduce((a, b) => a + b, 0) / xs.length : null;

interface Summary {
  spearman: number | null;
  pCheap: number | null;
  pPeak: number | null;
  mae: number | null;
  rmse: number | null;
  bias: number | null;
  nOrigins: number;
  nQuarters: number;
}
const summarize = (acc: Accumulator): Summary => {
  const preds = acc.pairs.map((x) => x[0]);
  const acts = acc.pairs.map((x) => x[1]);
  return {
    spearman: mean(acc.spearman),
    pCheap: mean(acc.pCheap),
    pPeak: mean(acc.pPeak),
    mae: acc.pairs.length > 0 ? mae(preds, acts) : null,
    rmse: acc.pairs.length > 0 ? rmse(preds, acts) : null,
    bias: acc.pairs.length > 0 ? bias(preds, acts) : null,
    nOrigins: acc.nOrigins,
    nQuarters: acc.pairs.length,
  };
};

const fmt = (v: number | null, dec = 2): string =>
  v !== null ? v.toFixed(dec).padStart(7) : "n/a".padStart(7);

const row = (label: string, s: Summary): string =>
  `  ${label.padEnd(24)}${fmt(s.spearman)} ${fmt(s.pCheap)} ${fmt(s.pPeak)} ` +
  `${fmt(s.mae)} ${fmt(s.rmse)} ${fmt(s.bias)} ${String(s.nOrigins).padStart(5)}`;

/** Naive baseline series over the same quarters the model covers. */
const baselineSeries = (
  name: "last_week" | "yesterday" | "flat",
  keys: readonly string[],
  realized: ReadonlyMap<string, number>,
  censored: ReadonlyMap<string, number>,
  tMs: number,
): { start: string; price: number }[] => {
  if (name === "flat") {
    const known = [...censored.values()];
    if (known.length === 0) return [];
    const m = known.reduce((a, b) => a + b, 0) / known.length;
    return keys.map((k) => ({ start: k, price: m }));
  }
  const offsetMs = name === "last_week" ? 7 * DAY_MS : DAY_MS;
  const out: { start: string; price: number }[] = [];
  for (const k of keys) {
    const srcMs = ms(k) - offsetMs;
    if (srcMs >= tMs) continue; // source not yet observed at origin
    const v = realized.get(quarterKey(srcMs));
    if (v !== undefined) {
      out.push({ start: k, price: v });
    }
  }
  return out;
};

const score = (
  series: readonly { start: string; price: number }[],
  realized: ReadonlyMap<string, number>,
): Record<Horizon, [number, number, number][]> => {
  const buckets: Record<Horizon, [number, number, number][]> = {
    day1: [],
    day2: [],
    day3: [],
  };
  if (series.length === 0) {
    return buckets;
  }
  const seriesStartMs = ms(series[0]?.start ?? "");
  for (const point of series) {
    const act = realized.get(point.start);
    if (act === undefined) continue;
    const h = horizonOf(ms(point.start), seriesStartMs);
    if (h !== null) {
      buckets[h].push([ms(point.start), point.price, act]);
    }
  }
  return buckets;
};

const main = (): void => {
  const dirArgIdx = process.argv.indexOf("--data");
  const here = path.dirname(fileURLToPath(import.meta.url));
  const dataDir =
    dirArgIdx >= 0 && process.argv[dirArgIdx + 1] !== undefined
      ? (process.argv[dirArgIdx + 1] as string)
      : path.join(here, "backtest-data");
  const cheapN = 8;
  const peakN = 4;

  const fixture = loadFixture(dataDir);
  const realized = buildRealized(fixture.prices);

  const wind245 = fixture.fingrid["245"] ?? [];
  const wind75 = fixture.fingrid["75"] ?? [];
  const cons165 = fixture.fingrid["165"] ?? [];
  const cons124 = fixture.fingrid["124"] ?? [];

  // One UTC origin per local day at the publication hour.
  const priceMsList = fixture.prices.map((p) => ms(p.start));
  const lo = Math.min(...priceMsList);
  const hi = Math.max(...priceMsList);
  const origins: number[] = [];
  let day = Math.floor(lo / DAY_MS) * DAY_MS;
  while (day <= hi) {
    origins.push(day + (PUBLICATION_HOUR_UTC + 1) * HOUR_MS); // just after publication
    day += DAY_MS;
  }

  const model: Record<Horizon, Accumulator> = {
    day1: newAcc(),
    day2: newAcc(),
    day3: newAcc(),
  };
  const baselines: Record<string, Record<Horizon, Accumulator>> = {
    last_week: { day1: newAcc(), day2: newAcc(), day3: newAcc() },
    yesterday: { day1: newAcc(), day2: newAcc(), day3: newAcc() },
    flat: { day1: newAcc(), day2: newAcc(), day3: newAcc() },
  };

  let used = 0;
  for (const tMs of origins) {
    const censored = censorPrices(fixture.prices, tMs);
    const last = lastKnownPriceMs(censored);
    const seriesStartMs = last !== null ? last + QUARTER_MS : tMs;
    const seriesEndMs = seriesStartMs + FORECAST_DAYS * DAY_MS;

    const endKey = quarterKey(seriesEndMs - QUARTER_MS);
    if (!realized.has(endKey) && !realized.has(quarterKey(seriesStartMs))) {
      continue;
    }
    used++;

    const result = buildForecast(
      {
        spotPricesByKey: censored,
        windForecast: toRecords(censorForecast(wind245, tMs, 72), 245),
        windActual: toRecords(censorActuals(wind75, tMs), 75),
        consumptionForecast: toRecords(censorForecast(cons165, tMs, 24), 165),
        consumptionActual: toRecords(censorActuals(cons124, tMs), 124),
        seriesStartMs,
        seriesEndMs,
      },
      {},
    );

    const modelSeries = result.series.map((p) => ({
      start: p.start,
      price: p.estimatedSpotCentsKwh,
    }));
    const modelBuckets = score(modelSeries, realized);
    for (const h of HORIZONS) {
      accumulate(model[h], modelBuckets[h], cheapN, peakN);
    }

    const keys: string[] = [];
    const numQuarters = Math.trunc((seriesEndMs - seriesStartMs) / QUARTER_MS);
    for (let i = 0; i < numQuarters; i++) {
      keys.push(quarterKey(seriesStartMs + i * QUARTER_MS));
    }
    for (const name of ["last_week", "yesterday", "flat"] as const) {
      const bser = baselineSeries(name, keys, realized, censored, tMs);
      const buckets = score(bser, realized);
      for (const h of HORIZONS) {
        accumulate(baselines[name]?.[h] ?? newAcc(), buckets[h], cheapN, peakN);
      }
    }
  }

  const header =
    `  ${"".padEnd(24)}${"Spear".padStart(7)} ${`P@${String(cheapN)}c`.padStart(7)} ` +
    `${`P@${String(peakN)}p`.padStart(7)} ${"MAE".padStart(7)} ${"RMSE".padStart(7)} ` +
    `${"bias".padStart(7)} ${"orig".padStart(5)}`;

  console.log(`\nFI forecast backtest — ${String(used)} origins`);
  console.log(header);
  console.log("MODEL");
  for (const h of HORIZONS) {
    console.log(row(h, summarize(model[h])));
  }
  console.log("\nBASELINES");
  for (const name of ["last_week", "yesterday", "flat"] as const) {
    for (const h of HORIZONS) {
      console.log(
        row(`${name} ${h}`, summarize(baselines[name]?.[h] ?? newAcc())),
      );
    }
  }

  const m1 = summarize(model.day1);
  const lw1 = summarize(baselines.last_week?.day1 ?? newAcc());
  if (m1.spearman !== null && lw1.spearman !== null) {
    const verb = m1.spearman > lw1.spearman ? "beats" : "LOSES TO";
    console.log(
      `\nVERDICT: day1 rank — model ${verb} last-week baseline ` +
        `(Spearman ${m1.spearman.toFixed(2)} vs ${lw1.spearman.toFixed(2)}).`,
    );
  } else {
    console.log("\nVERDICT: insufficient data.");
  }
  console.log("");
};

main();

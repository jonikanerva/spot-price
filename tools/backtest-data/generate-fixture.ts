/**
 * Deterministic synthetic fixture generator for the forecast backtest.
 *
 * We do NOT commit real Fingrid/Nord Pool dumps (multi-MB, and the forecast is
 * about ranking behaviour, not reproducing a specific month). Instead this
 * generates a small, fully deterministic ~21-day fixture where spot price has a
 * genuine relationship to (consumption − wind) plus a daily rhythm and noise —
 * enough for the backtest to demonstrate the model beats naive baselines.
 *
 * Run once with: pnpm tsx tools/backtest-data/generate-fixture.ts
 * The output fixture.json is committed; the backtest reads it by FILE PATH with
 * no network: `pnpm backtest --data tools/backtest-data/fixture.json`.
 */
import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const QUARTER_MS = 15 * 60 * 1000;
const START = Date.parse("2026-03-02T00:00:00.000Z");
const DAYS = 21;
const QUARTERS = DAYS * 96;

// Deterministic LCG so the fixture is reproducible across machines.
let seed = 1234567;
const rand = (): number => {
  seed = (seed * 1103515245 + 12345) & 0x7fffffff;
  return seed / 0x7fffffff;
};

const iso = (ms: number): string => new Date(ms).toISOString();

interface Price {
  start: string;
  spotCentsKwh: number;
}
interface Fingrid {
  startTime: string;
  endTime: string;
  value: number;
}

const prices: Price[] = [];
const wind: Fingrid[] = [];
const cons: Fingrid[] = [];

const TRUE_SLOPE = 0.0018;
const TRUE_INTERCEPT = -1.5;

for (let q = 0; q < QUARTERS; q++) {
  const ms = START + q * QUARTER_MS;
  const hour = new Date(ms).getUTCHours();
  // Consumption: daily double-peak (morning + evening) + weekly weekday lift.
  const dayOfWeek = new Date(ms).getUTCDay();
  const weekday = dayOfWeek >= 1 && dayOfWeek <= 5 ? 1 : 0;
  const morning = Math.exp(-((hour - 8) ** 2) / 6);
  const evening = Math.exp(-((hour - 18) ** 2) / 6);
  const consumption =
    7000 + 3000 * (morning + evening) + 800 * weekday + 200 * (rand() - 0.5);
  // Wind: slow random walk, no daily/weekly cycle.
  const windMw = 3000 + 1500 * Math.sin(q / 50) + 500 * (rand() - 0.5);
  const residual = consumption - windMw;
  // Daily price rhythm the linear model alone can't capture (hour bias target).
  const rhythm = 2 * (morning + evening) - 1;
  const noise = 0.6 * (rand() - 0.5);
  const spot = TRUE_SLOPE * residual + TRUE_INTERCEPT + rhythm + noise;

  prices.push({ start: iso(ms), spotCentsKwh: Math.round(spot * 1000) / 1000 });
  wind.push({
    startTime: iso(ms),
    endTime: iso(ms + QUARTER_MS),
    value: Math.round(windMw * 10) / 10,
  });
  cons.push({
    startTime: iso(ms),
    endTime: iso(ms + QUARTER_MS),
    value: Math.round(consumption * 10) / 10,
  });
}

// Datasets: forecast (245/165) ≈ actual here (clean synthetic), actual (75/124)
// identical so the weekly extension has history to draw on.
const fixture = {
  prices,
  fingrid: {
    "245": wind,
    "75": wind,
    "165": cons,
    "124": cons,
  },
};

const here = path.dirname(fileURLToPath(import.meta.url));
writeFileSync(
  path.join(here, "fixture.json"),
  JSON.stringify(fixture),
  "utf-8",
);
console.log(
  `Wrote fixture.json: ${String(prices.length)} price quarters, ${String(wind.length)} wind, ${String(cons.length)} cons`,
);

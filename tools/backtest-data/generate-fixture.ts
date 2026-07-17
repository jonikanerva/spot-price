/**
 * Deterministic synthetic fixture generator for the forecast backtest.
 *
 * We do NOT commit real Fingrid/Nord Pool dumps. Since issue #80 the committed
 * fixture is deliberately TEST-SIZED (< ~100 KB): its only role is the parity
 * round-trip and the old-shape-degrade tests, NOT a scoreable backtest — the
 * real delta and artifact recalibration run against the DB (`--db`). So this
 * generates a compact NEW-SHAPE fixture: FI prices + Fingrid ACTUALS (75/124) +
 * per-issuance forecast VINTAGES (245/165) with a small 2-deep issuance ladder
 * (a stale early issuance and a fresh near-delivery one) whose values DIFFER, so
 * the sample exercises the vintage-selection path.
 *
 * Run once with: pnpm tsx tools/backtest-data/generate-fixture.ts
 * The output fixture.json is committed; the backtest reads it by FILE PATH with
 * no network: `pnpm backtest --data tools/backtest-data/fixture.json`.
 */
import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const START = Date.parse("2026-03-02T00:00:00.000Z");
const DAYS = 1; // ~82 KB at quarter resolution with a 2-deep vintage ladder
const QUARTERS = DAYS * 96;

// Deterministic LCG so the fixture is reproducible across machines.
let seed = 1234567;
const rand = (): number => {
  seed = (seed * 1103515245 + 12345) & 0x7fffffff;
  return seed / 0x7fffffff;
};

const iso = (ms: number): string => new Date(ms).toISOString();
const r1 = (x: number): number => Math.round(x * 10) / 10;

interface Price {
  start: string;
  spotCentsKwh: number;
}
interface Actual {
  startTime: string;
  endTime: string;
  value: number;
}
interface Vintage {
  issuedAt: string;
  startTime: string;
  endTime: string;
  value: number;
}

const prices: Price[] = [];
const wind75: Actual[] = [];
const cons124: Actual[] = [];
const wind245: Vintage[] = [];
const cons165: Vintage[] = [];

const TRUE_SLOPE = 0.0018;
const TRUE_INTERCEPT = -1.5;

/** Two issuance leads (hours before delivery): a stale early one and a fresh one. */
const LADDER_LEADS_H = [20, 1] as const;

for (let q = 0; q < QUARTERS; q++) {
  const ms = START + q * QUARTER_MS;
  const hour = new Date(ms).getUTCHours();
  const morning = Math.exp(-((hour - 8) ** 2) / 6);
  const evening = Math.exp(-((hour - 18) ** 2) / 6);
  const consumption = 7000 + 3000 * (morning + evening) + 200 * (rand() - 0.5);
  const windMw = 3000 + 1500 * Math.sin(q / 50) + 500 * (rand() - 0.5);
  const residual = consumption - windMw;
  const rhythm = 2 * (morning + evening) - 1;
  const noise = 0.6 * (rand() - 0.5);
  const spot = TRUE_SLOPE * residual + TRUE_INTERCEPT + rhythm + noise;

  prices.push({ start: iso(ms), spotCentsKwh: Math.round(spot * 1000) / 1000 });
  wind75.push({
    startTime: iso(ms),
    endTime: iso(ms + QUARTER_MS),
    value: r1(windMw),
  });
  cons124.push({
    startTime: iso(ms),
    endTime: iso(ms + QUARTER_MS),
    value: r1(consumption),
  });

  // Vintage ladder: earlier issuances carry a lead-scaled revision away from the
  // near-delivery value, so honest (stale) and leaked (fresh) selections differ.
  for (const leadH of LADDER_LEADS_H) {
    const issuedAt = iso(ms - leadH * HOUR_MS);
    const revision = leadH * (0.5 * (rand() - 0.5)); // grows with lead
    wind245.push({
      issuedAt,
      startTime: iso(ms),
      endTime: iso(ms + QUARTER_MS),
      value: r1(windMw + revision * 20),
    });
    cons165.push({
      issuedAt,
      startTime: iso(ms),
      endTime: iso(ms + QUARTER_MS),
      value: r1(consumption + revision * 10),
    });
  }
}

const fixture = {
  prices,
  fingridActuals: { "75": wind75, "124": cons124 },
  fingridForecastVintages: { "245": wind245, "165": cons165 },
};

const here = path.dirname(fileURLToPath(import.meta.url));
const outPath = path.join(here, "fixture.json");
writeFileSync(outPath, JSON.stringify(fixture), "utf-8");
console.log(
  `Wrote fixture.json: ${String(prices.length)} prices, ${String(
    wind75.length + cons124.length,
  )} actuals, ${String(wind245.length + cons165.length)} vintages`,
);

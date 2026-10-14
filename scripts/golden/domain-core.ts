// Writes the golden fixtures of the Rust domain core from the TypeScript
// modules in src/. The Rust tests in crates/domain/tests/ compare against them.
// Run: pnpm exec tsx scripts/golden/domain-core.ts
// The output is deterministic: inputs come from a seeded generator and fixed
// lists, and no case reads the ambient clock.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as prettier from "prettier";
import {
  applyContractTerms,
  buildPriceWindow,
  calculateTotalPrice,
  calculateTotalPrices,
  extractHourInTimeZone,
  findCheapestWindow,
  isNightHour,
} from "../../src/calculator.js";
import { eurMwhToCentsKwh } from "../../src/nordpool.js";
import {
  addDays,
  formatDateInTimeZone,
  formatDateTimeInTimeZone,
  formatUtcDate,
  getCurrentAndNextDate,
  getUtcRangeForLocalDate,
  getUtcRangeForLocalDateSpan,
} from "../../src/time.js";
import { SUPPORTED_TIMEZONES } from "../../src/areas.js";
import {
  CheapestQuerySchema,
  PriceHistoryQuerySchema,
  UserSettingsUpdateSchema,
} from "../../src/api-schemas.js";
import type { HourlyPrice, TotalPrice, UserSettings } from "../../src/types.js";

const OUT_DIR = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../crates/domain/tests/golden/core",
);

// --- Encoding ---------------------------------------------------------------

// A float is written as its shortest round-trip decimal, which a correctly
// rounding parser maps back to the same bits. "-0" keeps the sign of zero.
const bits = (x: number): string => (Object.is(x, -0) ? "-0" : String(x));

// Each row of a table becomes one space-separated line, so the files stay
// small after Prettier and each case reads as one line in a diff.
const compact = (value: unknown): unknown => {
  if (Array.isArray(value)) {
    const isTable =
      value.length > 0 &&
      value.every(
        (row) =>
          Array.isArray(row) &&
          row.every((cell) => typeof cell !== "object" || cell === null),
      );
    return isTable
      ? value.map((row: unknown[]) => row.map((cell) => String(cell)).join(" "))
      : value.map(compact);
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, inner]) => [key, compact(inner)]),
    );
  }
  return value;
};

const ms = (iso: string): number => new Date(iso).getTime();

// mulberry32: a small seeded generator, so each run writes the same cases.
const rng = (seed: number): (() => number) => {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

/** Run `fn` with `new Date()` and `Date.now()` pinned to `nowMs`. */
const withNow = <T>(nowMs: number, fn: () => T): T => {
  const RealDate = Date;
  class FixedDate extends RealDate {
    constructor(...args: [] | [string | number | Date]) {
      if (args.length === 0) {
        super(nowMs);
      } else {
        super(args[0]);
      }
    }
    static override now(): number {
      return nowMs;
    }
  }
  globalThis.Date = FixedDate as DateConstructor;
  try {
    return fn();
  } finally {
    globalThis.Date = RealDate;
  }
};

const issuesText = (
  issues: readonly { path: PropertyKey[]; message: string }[],
) =>
  issues
    .map((i) =>
      i.path.length > 0
        ? `${i.path.map(String).join(".")}: ${i.message}`
        : i.message,
    )
    .join("; ");

const write = async (name: string, value: unknown): Promise<void> => {
  const path = join(OUT_DIR, name);
  const text = await prettier.format(JSON.stringify(compact(value)), {
    parser: "json",
    filepath: path,
    ...(await prettier.resolveConfig(path)),
  });
  writeFileSync(path, text);
};

// --- Shared inputs ------------------------------------------------------------

const ZONES = [...SUPPORTED_TIMEZONES];

const SETTINGS: UserSettings[] = [];
{
  const base = {
    userId: "golden",
    timezone: "Europe/Helsinki",
    area: "FI",
  };
  const combos: [number, number, number, number, number, number, number][] = [
    // margin, transferDay, transferNight, tax, vat, nightStart, nightEnd
    [0.49, 2.92, 1.37, 2.82752, 25.5, 22, 7],
    [0.45, 3.02, 1.55, 2.79372, 25.5, 22, 7],
    [0, 0, 0, 0, 0, 22, 7],
    [0, 0, 0, 0, 25.5, 0, 0],
    [0.3, 4.1, 4.1, 2.253, 24, 1, 6],
    [1.234, 5.678, 0.111, 2.79372, 10, 23, 0],
    [0.5, 2.5, 1.2, 2.79372, 100, 7, 22],
    [0.0005, 0.0015, 0.0025, 0.0035, 25.5, 12, 12],
    [2.675, 1.005, 0.015, 0.1, 14, 0, 23],
    [0.1, 0.2, 0.3, 0.4, 25.5, 6, 1],
    [10, 20, 30, 40, 0, 21, 21],
    [0.49, 2.92, 1.37, 2.82752, 25.5, 23, 22],
  ];
  for (const [m, td, tn, tax, vat, ns, ne] of combos) {
    SETTINGS.push({
      ...base,
      marginCentsKwh: m,
      transferDayCentsKwh: td,
      transferNightCentsKwh: tn,
      taxCentsKwh: tax,
      vatPercent: vat,
      nightStartHour: ns,
      nightEndHour: ne,
    });
  }
}

const settingsRow = (s: UserSettings) => [
  bits(s.marginCentsKwh),
  bits(s.transferDayCentsKwh),
  bits(s.transferNightCentsKwh),
  bits(s.taxCentsKwh),
  bits(s.vatPercent),
  s.nightStartHour,
  s.nightEndHour,
];

// --- round3 and the JavaScript number helpers --------------------------------

const roundCases = (): string[][] => {
  const values: number[] = [
    0,
    -0,
    0.5,
    -0.5,
    1.5,
    -1.5,
    2.5,
    -2.5,
    0.49999999999999994,
    -0.49999999999999994,
    0.49999999999999994 / 1000,
    -0.0005,
    0.0005,
    -0.0004,
    1.0005,
    -1.0005,
    2.675,
    -2.675,
    1.255,
    14.1359686,
    0.0015,
    1.0045,
    8.675,
    1e-320,
    -1e-320,
    4503599627370495.5,
    4503599627370497,
    9007199254740993,
    -9007199254740993,
    1e21,
    -1e21,
    1.7976931348623157e308,
    -1.7976931348623157e308,
    123456.7895,
    -123456.7895,
    0.1 + 0.2,
  ];
  for (let k = -1000; k <= 1000; k += 1) {
    const exact = k / 1000 + 0.0005;
    values.push(exact, -exact);
    values.push(exact + Number.EPSILON * Math.abs(exact));
    values.push(exact - Number.EPSILON * Math.abs(exact));
  }
  const rand = rng(1);
  for (let i = 0; i < 2000; i += 1) {
    const magnitude = 10 ** Math.floor(rand() * 8 - 3);
    values.push((rand() * 2 - 1) * magnitude);
  }
  return values.map((x) => [
    bits(x),
    bits(Math.round(x)),
    bits(Math.round(x * 1000) / 1000),
    bits(eurMwhToCentsKwh(x)),
  ]);
};

const NUMBER_STRINGS: string[] = [
  "",
  " ",
  "0",
  "-0",
  "+0",
  "00",
  "007",
  "1",
  "-1",
  "+1",
  "1.",
  ".5",
  "-.5",
  "+.5",
  ".",
  "-",
  "+",
  "1e1",
  "1E1",
  "1e+1",
  "1e-1",
  "1.5e3",
  ".5e2",
  "5.e3",
  "1e",
  "e1",
  "1e1.5",
  "1_000",
  "1,5",
  "0x10",
  "0X1f",
  "0x",
  "-0x10",
  "+0x10",
  "0b101",
  "0B11",
  "0b2",
  "0o17",
  "0O7",
  "0o8",
  "0x1fffffffffffff",
  "0x20000000000001",
  "0x20000000000003",
  "0x3fffffffffffff8",
  "0x" + "f".repeat(256),
  "0x1" + "0".repeat(255),
  "0b" + "1".repeat(60),
  "Infinity",
  "+Infinity",
  "-Infinity",
  "infinity",
  "inf",
  "NaN",
  "nan",
  "1e400",
  "-1e400",
  "1e-400",
  "1e99999999999999999999",
  "1e-99999999999999999999",
  "4.9e-324",
  "2.4703282292062327e-324",
  "2.4703282292062328e-324",
  "1.7976931348623157e308",
  "1.7976931348623158e308",
  "1.7976931348623159e308",
  "0.1",
  "0.30000000000000004",
  "123456789012345678901234567890",
  "9007199254740993",
  " 12 ",
  "\t12\n",
  " 12﻿",
  "  12  ",
  "\u008512",
  "12\u0085",
  "᠎12",
  "　12　",
  "1 2",
  "12abc",
  "abc",
  "١٢",
  "１２",
  "12.5.6",
  "--1",
  "+-1",
  "0.0000001",
  "1e21",
  "1e-7",
  "12.",
  "0.000",
  "-0.000",
  "000.5e-0",
  "1" + "0".repeat(400),
  "0." + "0".repeat(400) + "1",
];

// Objects, not table rows: a text can hold spaces.
const numberStringCases = (): { text: string; value: string }[] =>
  NUMBER_STRINGS.map((text) => ({ text, value: bits(Number(text)) }));

const toStringCases = (): string[][] => {
  const values: number[] = [
    0,
    -0,
    1,
    -1,
    0.5,
    1e21,
    1e20,
    123456789012345680000,
    1e-7,
    1e-6,
    0.000001,
    0.0000012345,
    1.5e-7,
    12.5,
    -12.5,
    0.1 + 0.2,
    5e-324,
    1.7976931348623157e308,
    2 ** 53,
    2 ** 70,
    100,
    1440,
    9.95,
    14.136,
    1 / 3,
    -1 / 3,
    1234.5678e30,
    4.35,
    0.07,
  ];
  const rand = rng(2);
  for (let i = 0; i < 3000; i += 1) {
    const magnitude = 10 ** Math.floor(rand() * 60 - 30);
    values.push((rand() * 2 - 1) * magnitude);
  }
  // Doubles from random bit patterns cover every exponent.
  const pattern = new DataView(new ArrayBuffer(8));
  for (let i = 0; i < 3000; i += 1) {
    pattern.setUint32(0, Math.floor(rand() * 2 ** 32));
    pattern.setUint32(4, Math.floor(rand() * 2 ** 32));
    const x = pattern.getFloat64(0);
    if (Number.isFinite(x)) values.push(x);
  }
  return values.map((x) => [bits(x), String(x)]);
};

// --- Contract terms -------------------------------------------------------------

const SPOTS_C_KWH: number[] = [
  -50, -20, -3, -1, -0.5, -0.0005, -0.0001, -0, 0, 0.0001, 0.0004, 0.0005,
  0.0015, 0.001, 0.5, 1, 1.0005, 2.675, 3, 5, 5.2345, 8.6745, 12.3455, 50.5,
  99.9995, 250, 1000.0005,
];

// The hours next to each night-window edge in SETTINGS, plus midday.
const CONTRACT_HOURS = [0, 1, 5, 6, 7, 11, 12, 13, 20, 21, 22, 23];

const contractCases = () => {
  const rows: (string | number | boolean)[][] = [];
  for (let s = 0; s < SETTINGS.length; s += 1) {
    const settings = SETTINGS[s];
    if (!settings) throw new Error("missing settings");
    for (let p = 0; p < SPOTS_C_KWH.length; p += 1) {
      const spot = SPOTS_C_KWH[p] ?? 0;
      for (const hour of CONTRACT_HOURS) {
        const b = applyContractTerms(spot, settings, hour);
        rows.push([
          s,
          bits(spot),
          hour,
          bits(b.spotCentsKwh),
          bits(b.transferCentsKwh),
          bits(b.vatCentsKwh),
          bits(b.totalCentsKwh),
          b.isNightRate,
        ]);
      }
    }
  }
  // One row per window: start, end, and 24 flags for hours 0..23.
  const nightRows: (number | string)[][] = [];
  for (let start = 0; start < 24; start += 1) {
    for (let end = 0; end < 24; end += 1) {
      let flags = "";
      for (let hour = 0; hour < 24; hour += 1) {
        flags += isNightHour(hour, start, end) ? "1" : "0";
      }
      nightRows.push([start, end, flags]);
    }
  }
  return { rows, nightRows };
};

// --- Time -------------------------------------------------------------------------

const DST_DAYS = ["2025-03-30", "2025-10-26", "2026-03-29", "2026-10-25"];

const isoDay = (msValue: number): string =>
  new Date(msValue).toISOString().slice(0, 10);

const everyDay = (from: string, to: string): string[] => {
  const out: string[] = [];
  for (
    let t = ms(`${from}T00:00:00Z`);
    t <= ms(`${to}T00:00:00Z`);
    t += 86_400_000
  ) {
    out.push(isoDay(t));
  }
  return out;
};

const localDayCases = () => {
  const days = [
    ...DST_DAYS.slice(0, 2).flatMap((d) =>
      everyDay(
        isoDay(ms(`${d}T00:00:00Z`) - 2 * 86_400_000),
        isoDay(ms(`${d}T00:00:00Z`) + 2 * 86_400_000),
      ),
    ),
    ...everyDay("2026-01-01", "2026-12-31"),
  ];
  const extraDays = [
    "2024-02-29",
    "2028-02-29",
    "2000-02-29",
    "2099-12-31",
    "1999-12-31",
  ];
  const ranges: (string | number)[][] = [];
  for (const zone of [...ZONES, "UTC"]) {
    for (const day of [...days, ...extraDays]) {
      const r = getUtcRangeForLocalDate(day, zone);
      ranges.push([zone, day, ms(r.startUtc), ms(r.endUtc)]);
    }
  }
  const spans: (string | number)[][] = [];
  const rand = rng(3);
  for (const zone of ZONES) {
    for (let i = 0; i < 60; i += 1) {
      const from = days[Math.floor(rand() * days.length)] ?? "2025-01-01";
      const lengthDays = Math.floor(rand() * 31);
      const to = isoDay(ms(`${from}T00:00:00Z`) + lengthDays * 86_400_000);
      const r = getUtcRangeForLocalDateSpan(from, to, zone);
      spans.push([zone, from, to, ms(r.startUtc), ms(r.endUtc)]);
    }
    for (const dst of DST_DAYS) {
      for (const [from, to] of [
        [dst, dst],
        [isoDay(ms(`${dst}T00:00:00Z`) - 86_400_000), dst],
        [dst, isoDay(ms(`${dst}T00:00:00Z`) + 86_400_000)],
      ] as const) {
        const r = getUtcRangeForLocalDateSpan(from, to, zone);
        spans.push([zone, from, to, ms(r.startUtc), ms(r.endUtc)]);
      }
    }
  }
  return { ranges, spans };
};

/** Instants around each DST change: every 15 min for ±6 h, hourly for ±1 day. */
const instantsAroundDst = (): number[] => {
  const out = new Set<number>();
  for (const day of DST_DAYS) {
    const change = ms(`${day}T01:00:00Z`);
    for (
      let t = change - 6 * 3_600_000;
      t <= change + 6 * 3_600_000;
      t += 900_000
    ) {
      out.add(t);
    }
    for (
      let t = change - 24 * 3_600_000;
      t <= change + 24 * 3_600_000;
      t += 3_600_000
    ) {
      out.add(t);
    }
    out.add(change - 1);
    out.add(change + 999);
  }
  for (const t of [
    ms("2026-02-24T22:00:00Z"),
    ms("2026-06-30T21:59:59.999Z"),
    ms("2026-12-31T22:30:00Z"),
    ms("2026-12-31T23:30:00Z"),
    ms("2027-01-01T00:00:00Z"),
    ms("2024-02-29T12:00:00Z"),
    ms("1970-01-01T00:00:00Z"),
    ms("2038-01-19T03:14:08Z"),
  ]) {
    out.add(t);
  }
  return [...out].sort((a, b) => a - b);
};

const formatCases = () => {
  const instants = instantsAroundDst();
  const rows: (string | number)[][] = [];
  for (const zone of [...ZONES, "UTC"]) {
    for (const t of instants) {
      const iso = new Date(t).toISOString();
      rows.push([
        zone,
        t,
        formatDateTimeInTimeZone(iso, zone),
        extractHourInTimeZone(iso, zone),
        formatDateInTimeZone(new Date(t), zone),
      ]);
    }
  }
  const utc: (string | number)[][] = instants.map((t) => [
    t,
    formatUtcDate(new Date(t)),
    addDays(new Date(t), 1).getTime(),
    addDays(new Date(t), -1).getTime(),
    addDays(new Date(t), 31).getTime(),
  ]);
  const today: string[][] = [];
  for (const zone of ZONES) {
    for (const t of instants) {
      const r = withNow(t, () => getCurrentAndNextDate(zone));
      today.push([zone, String(t), r.today, r.tomorrow]);
    }
  }
  return { rows, utc, today };
};

// --- Price math over intervals -------------------------------------------------------

const intervalsAround = (
  startIso: string,
  count: number,
  minutes: number,
  seed: number,
) => {
  const rand = rng(seed);
  const start = ms(startIso);
  const out: HourlyPrice[] = [];
  for (let i = 0; i < count; i += 1) {
    const s = start + i * minutes * 60_000;
    const raw = (rand() * 2 - 0.4) * 150;
    // Some prices keep 2 decimals like Nord Pool; some keep full precision.
    const price = i % 3 === 0 ? Math.round(raw * 100) / 100 : raw;
    out.push({
      deliveryStart: new Date(s).toISOString(),
      deliveryEnd: new Date(s + minutes * 60_000).toISOString(),
      priceEurMwh: price,
      area: "FI",
    });
  }
  return out;
};

const totalPriceCases = () => {
  const rows: (string | number | boolean)[][] = [];
  const windows: (string | number)[][] = [];
  const series: [string, number, number][] = [
    ["2026-03-28T20:00:00Z", 48, 15],
    ["2026-10-24T20:00:00Z", 48, 15],
    ["2025-03-29T20:00:00Z", 30, 60],
    ["2025-10-25T20:00:00Z", 30, 60],
    ["2026-02-23T22:00:00Z", 96, 15],
    ["2026-07-14T21:00:00Z", 24, 60],
  ];
  let seed = 10;
  for (let z = 0; z < ZONES.length; z += 1) {
    const zone = ZONES[z] ?? "Europe/Helsinki";
    for (let s = 0; s < SETTINGS.length; s += 1) {
      const settings = SETTINGS[s];
      if (!settings) throw new Error("missing settings");
      const zoned: UserSettings = { ...settings, timezone: zone };
      // Each zone and settings pair gets one series, so the file stays small.
      const pick = series[(z + s) % series.length];
      if (!pick) throw new Error("missing series");
      {
        const [startIso, count, minutes] = pick;
        seed += 1;
        const prices = intervalsAround(startIso, count, minutes, seed);
        const totals = calculateTotalPrices(prices, zoned);
        // Per-interval rows only for the first three settings; the window
        // row below covers the rest.
        for (let i = 0; i < (s < 3 ? prices.length : 0); i += 1) {
          const p = prices[i];
          const t = totals[i];
          if (!p || !t) throw new Error("missing price");
          rows.push([
            zone,
            s,
            ms(p.deliveryStart),
            ms(p.deliveryEnd),
            bits(p.priceEurMwh),
            t.localStart,
            t.localEnd,
            bits(t.spotCentsKwh),
            bits(t.transferCentsKwh),
            bits(t.vatCentsKwh),
            bits(t.totalCentsKwh),
            t.isNightRate,
          ]);
        }
        const w = buildPriceWindow(totals);
        if (!w) throw new Error("missing window");
        windows.push([
          zone,
          s,
          seed,
          startIso,
          count,
          minutes,
          ms(w.start),
          ms(w.end),
          w.startLocal,
          w.endLocal,
          bits(w.minTotalCentsKwh),
          bits(w.maxTotalCentsKwh),
          bits(w.averageTotalCentsKwh),
          prices.map((p) => bits(p.priceEurMwh)).join(","),
        ]);
      }
    }
  }
  // A single calculateTotalPrice check on an input with an explicit offset.
  const single = calculateTotalPrice(
    {
      deliveryStart: "2026-02-24T12:00:00+02:00",
      deliveryEnd: "2026-02-24T13:00:00+02:00",
      priceEurMwh: 50,
      area: "FI",
    },
    SETTINGS[1] ?? (SETTINGS[0] as UserSettings),
  );
  return {
    rows,
    windows,
    single: [single.localStart, single.localEnd, bits(single.totalCentsKwh)],
  };
};

// --- Cheapest window -------------------------------------------------------------------

interface CheapestCase {
  name: string;
  // "startMs,minutes,total" per entry, entries separated by ";".
  entries: string;
  duration: number;
  result: null | {
    first: number;
    count: number;
    start: number;
    end: number;
    min: string;
    max: string;
    avg: string;
  };
}

const fakeTotal = (
  startMs: number,
  endMs: number,
  total: number,
): TotalPrice => ({
  deliveryStart: new Date(startMs).toISOString(),
  deliveryEnd: new Date(endMs).toISOString(),
  localStart: "",
  localEnd: "",
  spotCentsKwh: total,
  marginCentsKwh: 0,
  transferCentsKwh: 0,
  taxCentsKwh: 0,
  vatCentsKwh: 0,
  totalCentsKwh: total,
  isNightRate: false,
});

const cheapestCase = (
  name: string,
  totals: TotalPrice[],
  duration: number,
): CheapestCase => {
  const w = findCheapestWindow(totals, duration);
  let result: CheapestCase["result"] = null;
  if (w) {
    const first = totals.findIndex((t) => t.deliveryStart === w.start);
    result = {
      first,
      count: w.prices.length,
      start: ms(w.start),
      end: ms(w.end),
      min: bits(w.minTotalCentsKwh),
      max: bits(w.maxTotalCentsKwh),
      avg: bits(w.averageTotalCentsKwh),
    };
  }
  return {
    name,
    entries: totals
      .map(
        (t) =>
          `${String(ms(t.deliveryStart))},${String((ms(t.deliveryEnd) - ms(t.deliveryStart)) / 60_000)},${bits(t.totalCentsKwh)}`,
      )
      .join(";"),
    duration,
    result,
  };
};

const series = (
  startMs: number,
  minutes: number,
  values: number[],
): TotalPrice[] =>
  values.map((v, i) =>
    fakeTotal(
      startMs + i * minutes * 60_000,
      startMs + (i + 1) * minutes * 60_000,
      v,
    ),
  );

const cheapestCases = (): CheapestCase[] => {
  const base = ms("2026-02-24T00:00:00Z");
  const cases: CheapestCase[] = [];
  cases.push(cheapestCase("empty", [], 60));
  cases.push(
    cheapestCase("hourly-3h", series(base, 60, [10, 2, 1, 3, 15]), 180),
  );
  cases.push(cheapestCase("too-short", series(base, 60, [5, 3]), 180));
  cases.push(cheapestCase("single-hour", series(base, 60, [10, 3, 8]), 60));
  cases.push(cheapestCase("exact-all", series(base, 60, [5, 3, 7]), 180));
  cases.push(
    cheapestCase(
      "ties-first-wins",
      series(base, 60, [4, 2, 2, 4, 2, 2, 4]),
      120,
    ),
  );
  cases.push(
    cheapestCase(
      "all-equal",
      series(base, 15, Array(16).fill(3) as number[]),
      60,
    ),
  );
  cases.push(
    cheapestCase(
      "negative",
      series(base, 60, [5, -2, -3, 1, 10, -1, -4, 2]),
      120,
    ),
  );
  cases.push(
    cheapestCase(
      "quarter-180",
      series(base, 15, [10, 10, 10, 10, 2, 2, 2, 2, 1, 1, 1, 1, 9, 9, 9, 9]),
      180,
    ),
  );
  cases.push(cheapestCase("round-up-50", series(base, 15, [5, 1, 2, 8]), 50));
  cases.push(
    cheapestCase(
      "round-up-280",
      series(
        base,
        15,
        [
          10, 10, 10, 10, 10, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
          10, 10,
        ],
      ),
      280,
    ),
  );
  cases.push(cheapestCase("one-minute", series(base, 15, [3, 1, 2]), 1));
  cases.push(
    cheapestCase(
      "full-day-1440",
      series(
        base,
        15,
        Array.from({ length: 100 }, (_, i) => (i * 7) % 11),
      ),
      1440,
    ),
  );
  // A gap of one quarter splits the series; the window must not span it.
  const gapped = [
    ...series(base, 15, [1, 1, 9, 9]),
    ...series(base + 75 * 60_000, 15, [0, 0, 0, 0]),
  ];
  cases.push(cheapestCase("gap-not-spanned", gapped, 60));
  cases.push(cheapestCase("gap-too-short", gapped, 90));
  // Hourly entries followed by quarter entries: mixed lengths are weighted.
  const mixed = [
    ...series(base, 60, [2, 8]),
    ...series(base + 120 * 60_000, 15, [1, 1, 1, 30, 1, 1]),
  ];
  for (const d of [15, 45, 60, 61, 75, 90, 120, 135, 180, 210]) {
    cases.push(cheapestCase(`mixed-${String(d)}`, mixed, d));
  }
  const rand = rng(4);
  for (let i = 0; i < 200; i += 1) {
    const minutes = rand() < 0.3 ? 60 : 15;
    const count = 1 + Math.floor(rand() * (minutes === 60 ? 48 : 192));
    let t = base + Math.floor(rand() * 96) * 900_000;
    const totals: TotalPrice[] = [];
    for (let k = 0; k < count; k += 1) {
      if (rand() < 0.03) {
        t += (1 + Math.floor(rand() * 4)) * 900_000;
      }
      const v =
        rand() < 0.2
          ? Math.round(rand() * 10)
          : Math.round((rand() * 40 - 5) * 1000) / 1000;
      totals.push(fakeTotal(t, t + minutes * 60_000, v));
      t += minutes * 60_000;
    }
    const duration =
      rand() < 0.5
        ? (1 + Math.floor(rand() * 16)) * 15
        : 1 + Math.floor(rand() * 1440);
    // The route drops entries above maxPrice before the scan, which leaves gaps.
    const maxPrice = rand() < 0.4 ? rand() * 30 : undefined;
    const kept =
      maxPrice === undefined
        ? totals
        : totals.filter((e) => e.totalCentsKwh <= maxPrice);
    cases.push(cheapestCase(`random-${String(i)}`, kept, duration));
  }
  return cases;
};

// --- Query parsing ---------------------------------------------------------------------

type QueryInput = Record<string, string>;

const cheapestQueryCases = () => {
  const durations = [
    undefined,
    "",
    " ",
    "0",
    "1",
    "60",
    "1440",
    "1441",
    "-5",
    "-0",
    "0.5",
    "1440.5",
    "60.0",
    "6e1",
    ".6e2",
    "60.",
    " 0x3C ",
    "0b111100",
    "0o74",
    "-0x3C",
    "abc",
    "1_0",
    "Infinity",
    "-Infinity",
    "1e400",
    "9007199254740993",
    "-9007199254740993",
    "1e20",
    "-1e20",
    " 60﻿",
    "\u008560",
    "NaN",
  ];
  const instants = [
    undefined,
    "",
    "x",
    "2026-02-27T14:00:00Z",
    "2026-02-27T14:00:00 02:00",
    "2026-02-27T14:00:00+02:00",
    "2026-02-27T14:00:00-05:30",
    "2026-02-27T14:00Z",
    "2026-02-27T14:00+02:00",
    "2026-02-27T14:00:00.1Z",
    "2026-02-27T14:00:00.1234567Z",
    "2026-02-27T14:00:00.9999+02:00",
    "2026-02-27T14:00:00+0200",
    "2026-02-27T14:00:00+02",
    "2026-02-27T14:00:00",
    "2026-02-27",
    "2026-02-27t14:00:00Z",
    "2026-02-27T14:00:00z",
    "2026-02-27T24:00:00Z",
    "2026-02-27T23:59:60Z",
    "2026-02-29T00:00:00Z",
    "2024-02-29T00:00:00Z",
    "2000-02-29T00:00:00Z",
    "1900-02-29T00:00:00Z",
    "0000-02-29T00:00:00Z",
    "9999-12-31T23:59:59.999-23:59",
    "0000-01-01T00:00:00+23:59",
    "2026-02-27T14:00:00+24:00",
    "2026-02-27T14:00:00+23:60",
    "2026-02-27T14:00:00.Z",
    "2026-02-27T14:00:00,5Z",
    " 2026-02-27T14:00:00Z",
    "2026-02-27T14:00:00Z ",
    "2026-02-27T14:00:00  02:00",
    "+2026-02-27T14:00:00Z",
    "2026-2-27T14:00:00Z",
    "2026-02-27T14:00:00.000000000000001Z",
    "２０２６-02-27T14:00:00Z",
  ];
  const prices = [
    undefined,
    "",
    "  ",
    "12",
    " 12 ",
    "-1",
    "0",
    "-0",
    "1e1",
    "0x10",
    ".5",
    "5.",
    "Infinity",
    "-Infinity",
    "1e400",
    "abc",
    "1,5",
    " 7﻿",
    "\u00857",
    "0.30000000000000004",
    "1e21",
    "1e-7",
    "123456789012345678901234567890",
  ];
  const inputs: QueryInput[] = [];
  for (const duration of durations) {
    const q: QueryInput = {};
    if (duration !== undefined) q.duration = duration;
    inputs.push(q);
  }
  for (const t of instants) {
    const q: QueryInput = { duration: "60" };
    if (t !== undefined) q.startTime = t;
    inputs.push(q);
    const r: QueryInput = { duration: "60" };
    if (t !== undefined) r.endTime = t;
    inputs.push(r);
  }
  for (const p of prices) {
    const q: QueryInput = { duration: "60" };
    if (p !== undefined) q.maxPrice = p;
    inputs.push(q);
  }
  inputs.push({ duration: "x", startTime: "y", endTime: "z", maxPrice: "w" });
  inputs.push({
    duration: "0.5",
    startTime: "2026-02-27T14:00:00Z",
    maxPrice: " ",
  });
  inputs.push({ startTime: "bad", maxPrice: "Infinity" });
  inputs.push({ duration: "1e20", endTime: "bad" });
  inputs.push({
    duration: "180",
    startTime: "2026-02-27T14:00:00 02:00",
    endTime: "2026-02-28T06:00:00 02:00",
    maxPrice: "12",
  });
  return inputs.map((q) => {
    const r = CheapestQuerySchema.safeParse(q);
    if (!r.success) {
      return { input: q, error: issuesText(r.error.issues) };
    }
    return {
      input: q,
      ok: {
        duration: r.data.duration,
        startTime: r.data.startTime === undefined ? null : ms(r.data.startTime),
        endTime: r.data.endTime === undefined ? null : ms(r.data.endTime),
        maxPrice: r.data.maxPrice === undefined ? null : bits(r.data.maxPrice),
      },
    };
  });
};

const historyQueryCases = () => {
  const pairs: [string | undefined, string | undefined][] = [
    ["2026-04-01", "2026-04-15"],
    ["2026-04-01", "2026-04-01"],
    ["2026-05-02", "2026-05-01"],
    ["2026-04-01", "2026-04-30"],
    ["2026-04-01", "2026-05-01"],
    ["2026-04-01", "2026-05-02"],
    ["2026-04-01T00:00:00Z", "2026-04-15"],
    ["2026-04-01", "2026-04-15T12:00:00+02:00"],
    ["not-a-date", "2026-04-15"],
    ["2026-13-40", "2026-05-01"],
    ["2026-04-01", ""],
    ["2026-04-01", undefined],
    [undefined, "2026-04-01"],
    [undefined, undefined],
    ["x", "y"],
    ["", ""],
    ["y", "x"],
    ["9", "1"],
    ["2026-04-31", "2026-06-01"],
    ["2026-02-30", "2026-03-01"],
    ["2026-02-29", "2026-03-01"],
    ["2024-02-29", "2024-03-30"],
    ["2024-02-29", "2024-03-31"],
    ["2100-02-29", "2100-03-01"],
    ["2000-02-29", "2000-03-01"],
    ["0001-01-01", "0100-01-01"],
    ["0099-12-31", "0100-01-01"],
    ["0000-01-01", "0000-01-31"],
    ["0000-02-29", "0000-03-01"],
    ["0050-01-01", "0050-03-01"],
    ["9999-12-01", "9999-12-31"],
    ["2026-12-31", "2027-01-31"],
    ["2026-12-31", "2027-02-01"],
    ["2025-10-01", "2025-10-31"],
    ["2026-04-01 ", "2026-04-02"],
    [" 2026-04-01", "2026-04-02"],
    ["2026-4-01", "2026-04-02"],
    ["2026-04-01", "2026-04-0١"],
    [" 2026-04-01", "2026-04-02"],
    ["2026-04-01", "2026-05-1x"],
    ["2026-0x-01", "2026-04-02"],
    ["1e10-01-01", "2026-01-01"],
    ["2026-01-01", "2026-03-0\u{1F600}"],
    ["\u{1F600}", "\u{1F601}"],
    ["", "\u{10000}"],
    ["2026-1.-01", "2026-03-01"],
    ["2026-01-01", "2026-01-.5"],
    ["-202-01-01", "2026-01-01"],
    ["+202-01-01", "2026-01-01"],
    ["2026--1-01", "2026-01-01"],
    ["2026-01- 1", "2026-01-31"],
  ];
  return pairs.map(([from, to]) => {
    const q: QueryInput = {};
    if (from !== undefined) q.from = from;
    if (to !== undefined) q.to = to;
    const r = PriceHistoryQuerySchema.safeParse(q);
    return r.success
      ? { input: q, ok: r.data }
      : { input: q, error: issuesText(r.error.issues) };
  });
};

const settingsUpdateCases = () => {
  const numbers = [
    0, -0, 0.5, -0.0001, 2.79372, 100, 100.0001, 1e300, -1e300, 22, 23, 24, -1,
    7.5, 9007199254740992, -9007199254740992,
  ];
  const rows: unknown[] = [];
  for (const field of [
    "marginCentsKwh",
    "transferDayCentsKwh",
    "transferNightCentsKwh",
    "taxCentsKwh",
    "vatPercent",
    "nightStartHour",
    "nightEndHour",
  ]) {
    for (const value of numbers) {
      const r = UserSettingsUpdateSchema.safeParse({ [field]: value });
      rows.push({
        field,
        value: bits(value),
        error: r.success ? null : issuesText(r.error.issues),
      });
    }
  }
  for (const field of ["timezone", "area"]) {
    for (const value of [
      "Europe/Helsinki",
      "FI",
      "SE3",
      "UTC",
      "",
      "europe/helsinki",
      "fi",
      "GER",
      "DE",
      "Europe/Berlin",
    ]) {
      const r = UserSettingsUpdateSchema.safeParse({ [field]: value });
      rows.push({
        field,
        text: value,
        error: r.success ? null : issuesText(r.error.issues),
      });
    }
  }
  const all = UserSettingsUpdateSchema.safeParse({
    marginCentsKwh: -1,
    vatPercent: 101,
    nightStartHour: 1.5,
    nightEndHour: 24,
    timezone: "UTC",
    area: "XX",
  });
  rows.push({
    field: "*",
    error: all.success ? null : issuesText(all.error.issues),
  });
  return rows;
};

// --- Main ------------------------------------------------------------------------------

mkdirSync(OUT_DIR, { recursive: true });
const contract = contractCases();
const days = localDayCases();
const formats = formatCases();
const totals = totalPriceCases();

await write("round3.json", {
  columns: ["x", "Math.round(x)", "round3(x)", "eurMwhToCentsKwh(x)"],
  rows: roundCases(),
});
await write("js_number.json", {
  number: numberStringCases(),
  toStringColumns: ["x", "String(x)"],
  toString: toStringCases(),
});
await write("contract_terms.json", {
  settingsColumns: [
    "margin",
    "transferDay",
    "transferNight",
    "tax",
    "vat",
    "nightStart",
    "nightEnd",
  ],
  settings: SETTINGS.map(settingsRow),
  columns: [
    "settings",
    "spot",
    "hour",
    "spotOut",
    "transfer",
    "vat",
    "total",
    "night",
  ],
  rows: contract.rows,
  nightColumns: ["start", "end", "isNight for hours 0..23"],
  night: contract.nightRows,
});
await write("local_days.json", {
  rangeColumns: ["zone", "date", "startMs", "endMs"],
  ranges: days.ranges,
  spanColumns: ["zone", "from", "to", "startMs", "endMs"],
  spans: days.spans,
});
await write("local_format.json", {
  columns: [
    "zone",
    "ms",
    "formatDateTimeInTimeZone",
    "extractHourInTimeZone",
    "formatDateInTimeZone",
  ],
  rows: formats.rows,
  utcColumns: ["ms", "formatUtcDate", "addDays+1", "addDays-1", "addDays+31"],
  utc: formats.utc,
  todayColumns: ["zone", "nowMs", "today", "tomorrow"],
  today: formats.today,
});
await write("total_prices.json", {
  settingsColumns: [
    "margin",
    "transferDay",
    "transferNight",
    "tax",
    "vat",
    "nightStart",
    "nightEnd",
  ],
  settings: SETTINGS.map(settingsRow),
  columns: [
    "zone",
    "settings",
    "startMs",
    "endMs",
    "eurMwh",
    "localStart",
    "localEnd",
    "spot",
    "transfer",
    "vat",
    "total",
    "night",
  ],
  rows: totals.rows,
  windowColumns: [
    "zone",
    "settings",
    "seed",
    "seriesStart",
    "count",
    "minutes",
    "startMs",
    "endMs",
    "startLocal",
    "endLocal",
    "min",
    "max",
    "avg",
    "eurMwh list",
  ],
  windows: totals.windows,
  single: totals.single,
});
await write("cheapest.json", { cases: cheapestCases() });
await write("queries.json", {
  cheapest: cheapestQueryCases(),
  history: historyQueryCases(),
  settingsUpdate: settingsUpdateCases(),
});

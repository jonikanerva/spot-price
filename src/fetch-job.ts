import type Database from "better-sqlite3";
import { fetchDayAheadPrices } from "./nordpool.js";
import { storePrices, countPricesByRange } from "./price-store.js";
import { DELIVERY_AREAS } from "./areas.js";
import { formatUtcDate } from "./time.js";

/** Minimum expected price entries per area per day (DST days may have 23h = 92 entries) */
const MIN_ENTRIES_PER_AREA = 23;

const getTodayAndTomorrow = (): { today: string; tomorrow: string } => {
  const now = new Date();
  const tomorrow = new Date(now);
  tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
  return {
    today: formatUtcDate(now),
    tomorrow: formatUtcDate(tomorrow),
  };
};

/**
 * Convert a UTC date (YYYY-MM-DD) to a UTC ISO range for database queries.
 * Nord Pool data for a given date spans from ~23:00 UTC previous day to ~23:00 UTC.
 * We use a full UTC day (00:00 to 24:00) for counting — this is an approximation
 * that works because the Nord Pool API returns data starting from 23:00 UTC prev day,
 * so the bulk of entries (92+) fall within the UTC day itself.
 */
const utcDateToRange = (
  utcDate: string,
): { startUtc: string; endUtc: string } => ({
  startUtc: `${utcDate}T00:00:00.000Z`,
  endUtc: `${utcDate}T23:59:59.999Z`,
});

interface FetchResult {
  readonly date: string;
  readonly stored: number;
  readonly skipped: boolean;
}

/** Check if all areas already have data for a given date */
const allAreasPresent = (db: Database.Database, date: string): boolean => {
  const totalAreas = DELIVERY_AREAS.length;
  const { startUtc, endUtc } = utcDateToRange(date);
  const areasWithData = DELIVERY_AREAS.filter(
    (a) =>
      countPricesByRange(db, startUtc, endUtc, a.code) >= MIN_ENTRIES_PER_AREA,
  ).length;
  return areasWithData >= totalAreas;
};

/** Fetch prices for all areas for a single date if not already complete */
const fetchForDate = async (
  db: Database.Database,
  date: string,
): Promise<FetchResult> => {
  if (allAreasPresent(db, date)) {
    return { date, stored: 0, skipped: true };
  }

  const allAreaCodes = DELIVERY_AREAS.map((a) => a.code);
  const prices = await fetchDayAheadPrices({ date, areas: allAreaCodes });
  if (prices.length === 0) {
    return { date, stored: 0, skipped: false };
  }

  const count = storePrices(db, prices);
  return { date, stored: count, skipped: false };
};

const logFetchResult = (date: string, result: FetchResult): void => {
  if (result.skipped) {
    console.log(
      `[fetch-job] ${date}: all ${String(DELIVERY_AREAS.length)} areas already in DB, skipped`,
    );
  } else if (result.stored === 0) {
    console.log(
      `[fetch-job] ${date}: not available yet (published ~14:00 EET)`,
    );
  } else {
    console.log(
      `[fetch-job] ${date}: stored ${String(result.stored)} prices across all areas`,
    );
  }
};

/** Run the daily price fetch job: fetch today + tomorrow for all areas */
export const runFetchJob = async (
  db: Database.Database,
): Promise<readonly FetchResult[]> => {
  const { today, tomorrow } = getTodayAndTomorrow();
  const results: FetchResult[] = [];

  console.log(
    `[fetch-job] Fetching prices for ${today} and ${tomorrow} (${String(DELIVERY_AREAS.length)} areas)...`,
  );

  const todayResult = await fetchForDate(db, today);
  results.push(todayResult);

  logFetchResult(today, todayResult);

  try {
    const tomorrowResult = await fetchForDate(db, tomorrow);
    results.push(tomorrowResult);
    logFetchResult(tomorrow, tomorrowResult);
  } catch (error) {
    // Network errors are not expected — log as warning
    const msg = error instanceof Error ? error.message : "unknown error";
    console.warn(`[fetch-job] ${tomorrow}: fetch failed (${msg})`);
  }

  return results;
};

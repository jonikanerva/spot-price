import type Database from "better-sqlite3";
import { fetchDayAheadPrices } from "./nordpool.js";
import { storePrices, countPricesForDate } from "./price-store.js";

const DEFAULT_AREA = "FI";

const formatDate = (date: Date): string => {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${String(year)}-${month}-${day}`;
};

const getTodayAndTomorrow = (): { today: string; tomorrow: string } => {
  const now = new Date();
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  return {
    today: formatDate(now),
    tomorrow: formatDate(tomorrow),
  };
};

interface FetchResult {
  readonly date: string;
  readonly stored: number;
  readonly skipped: boolean;
}

/** Fetch prices for a single date if not already in DB */
const fetchForDate = async (
  db: Database.Database,
  date: string,
  area: string,
): Promise<FetchResult> => {
  const existing = countPricesForDate(db, date, area);
  if (existing >= 23) {
    // At least 23 hours means we likely have the full day
    // (DST transition days have 23 or 25 hours)
    return { date, stored: 0, skipped: true };
  }

  const prices = await fetchDayAheadPrices({ date, area });
  if (prices.length === 0) {
    return { date, stored: 0, skipped: false };
  }

  const count = storePrices(db, prices);
  return { date, stored: count, skipped: false };
};

const logFetchResult = (date: string, result: FetchResult): void => {
  if (result.skipped) {
    console.log(`[fetch-job] ${date}: already in DB, skipped`);
  } else if (result.stored === 0) {
    console.log(
      `[fetch-job] ${date}: not available yet (published ~14:00 EET)`,
    );
  } else {
    console.log(`[fetch-job] ${date}: stored ${String(result.stored)} prices`);
  }
};

/** Run the daily price fetch job: fetch today + tomorrow */
export const runFetchJob = async (
  db: Database.Database,
): Promise<readonly FetchResult[]> => {
  const { today, tomorrow } = getTodayAndTomorrow();
  const results: FetchResult[] = [];

  console.log(`[fetch-job] Fetching prices for ${today} and ${tomorrow}...`);

  const todayResult = await fetchForDate(db, today, DEFAULT_AREA);
  results.push(todayResult);

  logFetchResult(today, todayResult);

  try {
    const tomorrowResult = await fetchForDate(db, tomorrow, DEFAULT_AREA);
    results.push(tomorrowResult);
    logFetchResult(tomorrow, tomorrowResult);
  } catch (error) {
    // Network errors are not expected — log as warning
    const msg = error instanceof Error ? error.message : "unknown error";
    console.warn(`[fetch-job] ${tomorrow}: fetch failed (${msg})`);
  }

  return results;
};

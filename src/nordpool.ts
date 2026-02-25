import type { HourlyPrice, NordPoolResponse } from "./types.js";

const BASE_URL = "https://dataportal-api.nordpoolgroup.com/api/DayAheadPrices";

interface FetchPricesParams {
  readonly date: string; // YYYY-MM-DD
  readonly area: string; // e.g. "FI"
}

/** Convert EUR/MWh to c/kWh (divide by 10) */
export const eurMwhToCentsKwh = (eurMwh: number): number =>
  Math.round((eurMwh / 10) * 1000) / 1000;

const buildUrl = (params: FetchPricesParams): string => {
  const url = new URL(BASE_URL);
  url.searchParams.set("date", params.date);
  url.searchParams.set("market", "DayAhead");
  url.searchParams.set("deliveryArea", params.area);
  url.searchParams.set("currency", "EUR");
  return url.toString();
};

const parseResponse = (
  data: NordPoolResponse,
  area: string,
): readonly HourlyPrice[] => {
  return data.multiAreaEntries
    .map((entry): HourlyPrice | null => {
      const price = entry.entryPerArea[area];
      if (price === undefined) {
        return null;
      }
      return {
        deliveryStart: entry.deliveryStart,
        deliveryEnd: entry.deliveryEnd,
        priceEurMwh: price,
        area,
      };
    })
    .filter((p): p is HourlyPrice => p !== null);
};

const MAX_RETRIES = 3;
const RETRY_DELAY_MS = 7000;

const delay = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

/** HTTP status codes that indicate "no data" rather than a transient error */
const NO_DATA_STATUSES = new Set([404, 204]);

/**
 * Parse JSON response body safely.
 * Returns null if the body is empty or not valid JSON (indicates no data).
 */
const parseJsonBody = async (
  response: Response,
): Promise<NordPoolResponse | null> => {
  const text = await response.text();
  if (text.length === 0) {
    return null;
  }
  try {
    return JSON.parse(text) as NordPoolResponse;
  } catch {
    return null;
  }
};

/** Fetch day-ahead prices from Nord Pool Data Portal API */
export const fetchDayAheadPrices = async (
  params: FetchPricesParams,
): Promise<readonly HourlyPrice[]> => {
  const url = buildUrl(params);

  for (let attempt = 1; attempt <= MAX_RETRIES; attempt++) {
    try {
      const response = await fetch(url);

      // "No data" responses — return empty array immediately, no retry
      if (NO_DATA_STATUSES.has(response.status)) {
        return [];
      }

      if (!response.ok) {
        throw new Error(
          `Nord Pool API returned ${String(response.status)}: ${response.statusText}`,
        );
      }

      const data = await parseJsonBody(response);
      if (!data) {
        // Empty or unparseable body — no data available, not a transient error
        return [];
      }

      return parseResponse(data, params.area);
    } catch (error) {
      const isLastAttempt = attempt === MAX_RETRIES;
      if (isLastAttempt) {
        throw error;
      }
      console.warn(
        `[nordpool] Fetch attempt ${String(attempt)}/${String(MAX_RETRIES)} failed, retrying in ${String(RETRY_DELAY_MS / 1000)}s...`,
      );
      await delay(RETRY_DELAY_MS);
    }
  }

  // Unreachable but satisfies TypeScript
  throw new Error("Max retries exceeded");
};

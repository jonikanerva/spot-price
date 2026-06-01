import type {
  HourlyPrice,
  UserSettings,
  TotalPrice,
  PriceWindow,
} from "./types.js";
import { eurMwhToCentsKwh } from "./nordpool.js";
import { formatDateTimeInTimeZone } from "./time.js";

/** Determine if a given hour falls within the night rate window */
export const isNightHour = (
  hour: number,
  nightStartHour: number,
  nightEndHour: number,
): boolean => {
  // Night window wraps around midnight (e.g., 22-07)
  if (nightStartHour > nightEndHour) {
    return hour >= nightStartHour || hour < nightEndHour;
  }
  // Night window within same day (unusual but supported, e.g., 01-06)
  return hour >= nightStartHour && hour < nightEndHour;
};

// An `Intl.DateTimeFormat` instance is stateless across the dates passed to
// `formatToParts`, so it can be reused. `calculateTotalPrices` invokes this for
// every interval (up to ~2976 at the price-history 31-day cap); memoising the
// formatter per timezone keeps construction O(timezones) instead of
// O(intervals) and stays inside the STACK.md §4 100 ms p99 budget.
const hourFormatterCache = new Map<string, Intl.DateTimeFormat>();

const getHourFormatter = (timeZone: string): Intl.DateTimeFormat => {
  const cached = hourFormatterCache.get(timeZone);
  if (cached) {
    return cached;
  }
  const formatter = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hour: "2-digit",
    hour12: false,
  });
  hourFormatterCache.set(timeZone, formatter);
  return formatter;
};

/** Extract hour (0-23) in a specific IANA timezone */
export const extractHourInTimeZone = (
  isoDateTime: string,
  timeZone: string,
): number => {
  const date = new Date(isoDateTime);
  const formatter = getHourFormatter(timeZone);
  const hourPart = formatter
    .formatToParts(date)
    .find((part) => part.type === "hour")?.value;

  if (!hourPart) {
    throw new Error(`Failed to parse hour for timezone ${timeZone}`);
  }

  const hour = Number.parseInt(hourPart, 10);
  if (!Number.isInteger(hour) || hour < 0 || hour > 23) {
    throw new Error(`Invalid hour '${hourPart}' parsed from ${isoDateTime}`);
  }
  return hour;
};

const getIntervalMinutes = (startIso: string, endIso: string): number => {
  const start = new Date(startIso).getTime();
  const end = new Date(endIso).getTime();
  const minutes = (end - start) / 60000;
  if (!Number.isFinite(minutes) || minutes <= 0) {
    throw new Error(`Invalid interval: ${startIso} -> ${endIso}`);
  }
  return minutes;
};

/** The contract-term breakdown applied on top of a spot price. */
export interface ContractBreakdown {
  readonly spotCentsKwh: number;
  readonly marginCentsKwh: number;
  readonly transferCentsKwh: number;
  readonly taxCentsKwh: number;
  readonly vatCentsKwh: number;
  readonly totalCentsKwh: number;
  readonly isNightRate: boolean;
}

/**
 * Apply the user's contract terms (margin + day/night transfer + electricity
 * tax + VAT) to a spot price in c/kWh, returning the full money breakdown.
 *
 * This is the single source of total-price truth: both the real-price path
 * (`calculateTotalPrice`) and the forecast route call it with a spot value in
 * c/kWh, so an estimated total is computed by exactly the same arithmetic as a
 * real total — the forecast never fabricates a `HourlyPrice`. `hour` is the
 * hour-of-day in the user's timezone, used only to pick the day/night transfer
 * rate; it is supplied by the caller so this function stays pure (no clock, no
 * timezone parsing inline) and reusable for estimated quarters.
 */
export const applyContractTerms = (
  spotCentsKwh: number,
  settings: UserSettings,
  hour: number,
): ContractBreakdown => {
  const nightRate = isNightHour(
    hour,
    settings.nightStartHour,
    settings.nightEndHour,
  );
  const transferCentsKwh = nightRate
    ? settings.transferNightCentsKwh
    : settings.transferDayCentsKwh;

  // Total before VAT: spot + margin + transfer + electricity tax
  const beforeVat =
    spotCentsKwh +
    settings.marginCentsKwh +
    transferCentsKwh +
    settings.taxCentsKwh;

  // VAT applied to the total
  const vatMultiplier = 1 + settings.vatPercent / 100;
  const totalCentsKwh = Math.round(beforeVat * vatMultiplier * 1000) / 1000;
  const vatCentsKwh =
    Math.round(beforeVat * (settings.vatPercent / 100) * 1000) / 1000;

  return {
    spotCentsKwh,
    marginCentsKwh: settings.marginCentsKwh,
    transferCentsKwh,
    taxCentsKwh: settings.taxCentsKwh,
    vatCentsKwh,
    totalCentsKwh,
    isNightRate: nightRate,
  };
};

/** Calculate total price for a single hour */
export const calculateTotalPrice = (
  price: HourlyPrice,
  settings: UserSettings,
): TotalPrice => {
  const spotCentsKwh = eurMwhToCentsKwh(price.priceEurMwh);
  const hour = extractHourInTimeZone(price.deliveryStart, settings.timezone);
  const breakdown = applyContractTerms(spotCentsKwh, settings, hour);

  return {
    deliveryStart: price.deliveryStart,
    deliveryEnd: price.deliveryEnd,
    localStart: formatDateTimeInTimeZone(
      price.deliveryStart,
      settings.timezone,
    ),
    localEnd: formatDateTimeInTimeZone(price.deliveryEnd, settings.timezone),
    ...breakdown,
  };
};

/** Calculate total prices for an array of hourly prices */
export const calculateTotalPrices = (
  prices: readonly HourlyPrice[],
  settings: UserSettings,
): TotalPrice[] => prices.map((p) => calculateTotalPrice(p, settings));

/**
 * Find the cheapest contiguous window of at least the given duration.
 * Supports variable interval lengths (e.g. 60 min, 15 min).
 * When duration is not an exact multiple of the interval, the window
 * is rounded up to the next interval boundary (e.g. 280 min with
 * 15-min data → 285-min window).
 *
 * Uses a contiguous-window scan — O(n^2), but n is small (<= 96/day).
 *
 * @param prices Sorted array of total prices
 * @param durationMinutes Minimum window length in minutes
 * @returns The cheapest window of at least durationMinutes, or null if none exists
 */
/** Build a PriceWindow from a non-empty array of TotalPrice entries */
export const buildPriceWindow = (prices: TotalPrice[]): PriceWindow | null => {
  if (prices.length === 0) {
    return null;
  }

  const firstPrice = prices[0];
  const lastPrice = prices[prices.length - 1];
  if (!firstPrice || !lastPrice) {
    return null;
  }

  let totalWeightedSum = 0;
  let totalMinutes = 0;
  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;

  for (const entry of prices) {
    const intervalMinutes = getIntervalMinutes(
      entry.deliveryStart,
      entry.deliveryEnd,
    );
    totalWeightedSum += entry.totalCentsKwh * intervalMinutes;
    totalMinutes += intervalMinutes;
    if (entry.totalCentsKwh < min) min = entry.totalCentsKwh;
    if (entry.totalCentsKwh > max) max = entry.totalCentsKwh;
  }

  const average = totalMinutes > 0 ? totalWeightedSum / totalMinutes : 0;

  return {
    start: firstPrice.deliveryStart,
    end: lastPrice.deliveryEnd,
    startLocal: firstPrice.localStart,
    endLocal: lastPrice.localEnd,
    minTotalCentsKwh: Math.round(min * 1000) / 1000,
    maxTotalCentsKwh: Math.round(max * 1000) / 1000,
    averageTotalCentsKwh: Math.round(average * 1000) / 1000,
    prices,
  };
};

export const findCheapestWindow = (
  prices: readonly TotalPrice[],
  durationMinutes: number,
): PriceWindow | null => {
  if (prices.length === 0 || durationMinutes <= 0) {
    return null;
  }

  let bestAverage = Number.POSITIVE_INFINITY;
  let bestWindow: TotalPrice[] | null = null;

  for (let startIndex = 0; startIndex < prices.length; startIndex++) {
    const window: TotalPrice[] = [];
    let accumulatedMinutes = 0;
    let weightedSum = 0;
    let previousEnd: string | null = null;

    for (let index = startIndex; index < prices.length; index++) {
      const entry = prices[index];
      if (!entry) {
        break;
      }

      if (previousEnd !== null && entry.deliveryStart !== previousEnd) {
        break;
      }

      const intervalMinutes = getIntervalMinutes(
        entry.deliveryStart,
        entry.deliveryEnd,
      );

      window.push(entry);
      accumulatedMinutes += intervalMinutes;
      weightedSum += entry.totalCentsKwh * intervalMinutes;
      previousEnd = entry.deliveryEnd;

      if (accumulatedMinutes >= durationMinutes) {
        const average = weightedSum / accumulatedMinutes;
        if (average < bestAverage) {
          bestAverage = average;
          bestWindow = [...window];
        }
        break;
      }
    }
  }

  if (!bestWindow || bestWindow.length === 0) {
    return null;
  }

  return buildPriceWindow(bestWindow);
};

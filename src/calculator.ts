import type {
  HourlyPrice,
  UserSettings,
  TotalPrice,
  CheapestWindow,
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

/** Extract hour (0-23) in a specific IANA timezone */
const extractHourInTimeZone = (
  isoDateTime: string,
  timeZone: string,
): number => {
  const date = new Date(isoDateTime);
  const formatter = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hour: "2-digit",
    hour12: false,
  });
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

/** Calculate total price for a single hour */
export const calculateTotalPrice = (
  price: HourlyPrice,
  settings: UserSettings,
): TotalPrice => {
  const spotCentsKwh = eurMwhToCentsKwh(price.priceEurMwh);
  const hour = extractHourInTimeZone(price.deliveryStart, settings.timezone);
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
    deliveryStart: price.deliveryStart,
    deliveryEnd: price.deliveryEnd,
    localStart: formatDateTimeInTimeZone(
      price.deliveryStart,
      settings.timezone,
    ),
    localEnd: formatDateTimeInTimeZone(price.deliveryEnd, settings.timezone),
    spotCentsKwh,
    marginCentsKwh: settings.marginCentsKwh,
    transferCentsKwh,
    taxCentsKwh: settings.taxCentsKwh,
    vatCentsKwh,
    totalCentsKwh,
    isNightRate: nightRate,
  };
};

/** Calculate total prices for an array of hourly prices */
export const calculateTotalPrices = (
  prices: readonly HourlyPrice[],
  settings: UserSettings,
): readonly TotalPrice[] => prices.map((p) => calculateTotalPrice(p, settings));

/**
 * Find the cheapest contiguous window of a given duration.
 * Supports variable interval lengths (e.g. 60 min, 15 min).
 *
 * Uses a contiguous-window scan — O(n^2), but n is small (<= 96/day).
 *
 * @param prices Sorted array of total prices
 * @param durationMinutes Desired exact window length in minutes
 * @returns The cheapest exact-duration window, or null if none exists
 */
export const findCheapestWindow = (
  prices: readonly TotalPrice[],
  durationMinutes: number,
): CheapestWindow | null => {
  if (prices.length === 0 || durationMinutes <= 0) {
    return null;
  }

  let bestAverage = Number.POSITIVE_INFINITY;
  let bestWindow: readonly TotalPrice[] | null = null;

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

      if (accumulatedMinutes === durationMinutes) {
        const average = weightedSum / accumulatedMinutes;
        if (average < bestAverage) {
          bestAverage = average;
          bestWindow = [...window];
        }
        break;
      }

      if (accumulatedMinutes > durationMinutes) {
        break;
      }
    }
  }

  if (!bestWindow || bestWindow.length === 0) {
    return null;
  }

  const firstPrice = bestWindow[0];
  const lastPrice = bestWindow[bestWindow.length - 1];

  if (!firstPrice || !lastPrice) {
    return null;
  }

  return {
    start: firstPrice.deliveryStart,
    end: lastPrice.deliveryEnd,
    startLocal: firstPrice.localStart,
    endLocal: lastPrice.localEnd,
    averageTotalCentsKwh: Math.round(bestAverage * 1000) / 1000,
    prices: bestWindow,
  };
};

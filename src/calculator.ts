import type {
  HourlyPrice,
  UserSettings,
  TotalPrice,
  CheapestWindow,
} from "./types.js";
import { eurMwhToCentsKwh } from "./nordpool.js";

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

/** Extract hour (0-23) from an ISO datetime string */
const extractHour = (isoDateTime: string): number => {
  const date = new Date(isoDateTime);
  // Use UTC hours and offset to get local hour
  // The delivery times from Nord Pool include timezone offset
  return date.getHours();
};

/** Calculate total price for a single hour */
export const calculateTotalPrice = (
  price: HourlyPrice,
  settings: UserSettings,
): TotalPrice => {
  const spotCentsKwh = eurMwhToCentsKwh(price.priceEurMwh);
  const hour = extractHour(price.deliveryStart);
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
    spotCentsKwh,
    marginCentsKwh: settings.marginCentsKwh,
    transferCentsKwh,
    taxCentsKwh: settings.taxCentsKwh,
    vatCentsKwh,
    totalCentsKwh,
    isNightRate: nightRate,
    hour,
  };
};

/** Calculate total prices for an array of hourly prices */
export const calculateTotalPrices = (
  prices: readonly HourlyPrice[],
  settings: UserSettings,
): readonly TotalPrice[] => prices.map((p) => calculateTotalPrice(p, settings));

/**
 * Find the cheapest contiguous window of a given duration.
 * Uses a sliding window algorithm — O(n) time complexity.
 *
 * @param prices - Sorted array of hourly total prices
 * @param durationMinutes - Desired window length in minutes (must be multiple of 60 for hourly data)
 * @returns The cheapest window, or null if not enough prices
 */
export const findCheapestWindow = (
  prices: readonly TotalPrice[],
  durationMinutes: number,
): CheapestWindow | null => {
  const windowSize = Math.ceil(durationMinutes / 60);

  if (prices.length === 0 || windowSize <= 0 || windowSize > prices.length) {
    return null;
  }

  // Calculate initial window sum
  let currentSum = 0;
  for (let i = 0; i < windowSize; i++) {
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    currentSum += prices[i]!.totalCentsKwh;
  }

  let bestSum = currentSum;
  let bestStartIndex = 0;

  // Slide the window
  for (let i = windowSize; i < prices.length; i++) {
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    currentSum += prices[i]!.totalCentsKwh;
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    currentSum -= prices[i - windowSize]!.totalCentsKwh;

    if (currentSum < bestSum) {
      bestSum = currentSum;
      bestStartIndex = i - windowSize + 1;
    }
  }

  const windowPrices = prices.slice(
    bestStartIndex,
    bestStartIndex + windowSize,
  );
  const firstPrice = windowPrices[0];
  const lastPrice = windowPrices[windowPrices.length - 1];

  if (!firstPrice || !lastPrice) {
    return null;
  }

  return {
    start: firstPrice.deliveryStart,
    end: lastPrice.deliveryEnd,
    averageTotalCentsKwh: Math.round((bestSum / windowSize) * 1000) / 1000,
    prices: windowPrices,
  };
};

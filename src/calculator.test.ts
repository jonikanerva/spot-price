import { describe, it, expect } from "vitest";
import {
  isNightHour,
  calculateTotalPrice,
  calculateTotalPrices,
  findCheapestWindow,
} from "./calculator.js";
import type { HourlyPrice, UserSettings, TotalPrice } from "./types.js";

const defaultSettings: UserSettings = {
  userId: "test-user",
  marginCentsKwh: 0.45,
  transferDayCentsKwh: 3.02,
  transferNightCentsKwh: 1.55,
  taxCentsKwh: 2.79372,
  vatPercent: 25.5,
  nightStartHour: 22,
  nightEndHour: 7,
  timezone: "Europe/Helsinki",
};

const makePrice = (hour: number, eurMwh: number): HourlyPrice => ({
  deliveryStart: `2026-02-24T${String(hour).padStart(2, "0")}:00:00+02:00`,
  deliveryEnd: `2026-02-24T${String(hour + 1).padStart(2, "0")}:00:00+02:00`,
  priceEurMwh: eurMwh,
  area: "FI",
});

describe("isNightHour", () => {
  it("returns true for hours within night window (22-07)", () => {
    expect(isNightHour(22, 22, 7)).toBe(true);
    expect(isNightHour(23, 22, 7)).toBe(true);
    expect(isNightHour(0, 22, 7)).toBe(true);
    expect(isNightHour(3, 22, 7)).toBe(true);
    expect(isNightHour(6, 22, 7)).toBe(true);
  });

  it("returns false for hours outside night window (22-07)", () => {
    expect(isNightHour(7, 22, 7)).toBe(false);
    expect(isNightHour(12, 22, 7)).toBe(false);
    expect(isNightHour(21, 22, 7)).toBe(false);
    expect(isNightHour(14, 22, 7)).toBe(false);
  });

  it("handles same-day night window (e.g., 01-06)", () => {
    expect(isNightHour(1, 1, 6)).toBe(true);
    expect(isNightHour(3, 1, 6)).toBe(true);
    expect(isNightHour(5, 1, 6)).toBe(true);
    expect(isNightHour(0, 1, 6)).toBe(false);
    expect(isNightHour(6, 1, 6)).toBe(false);
    expect(isNightHour(12, 1, 6)).toBe(false);
  });
});

describe("calculateTotalPrice", () => {
  it("calculates correct total for a day hour", () => {
    const price = makePrice(12, 50.0); // 5.0 c/kWh spot
    const result = calculateTotalPrice(price, defaultSettings);

    expect(result.spotCentsKwh).toBe(5.0);
    expect(result.marginCentsKwh).toBe(0.45);
    expect(result.transferCentsKwh).toBe(3.02); // day rate
    expect(result.taxCentsKwh).toBe(2.79372);
    expect(result.isNightRate).toBe(false);
    expect(result.hour).toBe(12);

    // Before VAT: 5.0 + 0.45 + 3.02 + 2.79372 = 11.26372
    // With 25.5% VAT: 11.26372 * 1.255 = 14.135969
    expect(result.totalCentsKwh).toBeCloseTo(14.136, 2);
    expect(result.vatCentsKwh).toBeCloseTo(2.872, 2);
  });

  it("calculates correct total for a night hour", () => {
    const price = makePrice(2, 30.0); // 3.0 c/kWh spot
    const result = calculateTotalPrice(price, defaultSettings);

    expect(result.spotCentsKwh).toBe(3.0);
    expect(result.transferCentsKwh).toBe(1.55); // night rate
    expect(result.isNightRate).toBe(true);
    expect(result.hour).toBe(2);

    // Before VAT: 3.0 + 0.45 + 1.55 + 2.79372 = 7.79372
    // With 25.5% VAT: 7.79372 * 1.255 = 9.781119
    expect(result.totalCentsKwh).toBeCloseTo(9.781, 2);
  });

  it("handles negative spot prices", () => {
    const price = makePrice(14, -10.0); // -1.0 c/kWh spot
    const result = calculateTotalPrice(price, defaultSettings);

    expect(result.spotCentsKwh).toBe(-1.0);
    // Before VAT: -1.0 + 0.45 + 3.02 + 2.79372 = 5.26372
    // Still positive total (transfer + tax + margin outweigh negative spot)
    expect(result.totalCentsKwh).toBeCloseTo(6.606, 2);
  });

  it("handles zero margin", () => {
    const settings: UserSettings = { ...defaultSettings, marginCentsKwh: 0 };
    const price = makePrice(12, 50.0);
    const result = calculateTotalPrice(price, settings);

    expect(result.marginCentsKwh).toBe(0);
    // Before VAT: 5.0 + 0 + 3.02 + 2.79372 = 10.81372
    // With 25.5% VAT: 10.81372 * 1.255 = 13.571219
    expect(result.totalCentsKwh).toBeCloseTo(13.571, 2);
  });
});

describe("calculateTotalPrices", () => {
  it("maps all prices correctly", () => {
    const prices = [makePrice(0, 20.0), makePrice(12, 80.0)];
    const results = calculateTotalPrices(prices, defaultSettings);

    expect(results).toHaveLength(2);
    expect(results[0]?.isNightRate).toBe(true);
    expect(results[1]?.isNightRate).toBe(false);
  });
});

describe("findCheapestWindow", () => {
  const makeTotalPrice = (hour: number, totalCentsKwh: number): TotalPrice => ({
    deliveryStart: `2026-02-24T${String(hour).padStart(2, "0")}:00:00+02:00`,
    deliveryEnd: `2026-02-24T${String(hour + 1).padStart(2, "0")}:00:00+02:00`,
    spotCentsKwh: totalCentsKwh,
    marginCentsKwh: 0,
    transferCentsKwh: 0,
    taxCentsKwh: 0,
    vatCentsKwh: 0,
    totalCentsKwh,
    isNightRate: false,
    hour,
  });

  it("finds the cheapest 3-hour window", () => {
    const prices: readonly TotalPrice[] = [
      makeTotalPrice(0, 10), // expensive
      makeTotalPrice(1, 2), // cheap window start
      makeTotalPrice(2, 1), // cheapest
      makeTotalPrice(3, 3), // cheap window end
      makeTotalPrice(4, 15), // expensive
    ];

    const result = findCheapestWindow(prices, 180);

    expect(result).not.toBeNull();
    expect(result?.prices).toHaveLength(3);
    expect(result?.start).toContain("T01:");
    expect(result?.end).toContain("T04:");
    expect(result?.averageTotalCentsKwh).toBe(2); // (2+1+3)/3
  });

  it("returns null when window is longer than available prices", () => {
    const prices = [makeTotalPrice(0, 5), makeTotalPrice(1, 3)];
    const result = findCheapestWindow(prices, 180);

    expect(result).toBeNull();
  });

  it("returns null for empty prices", () => {
    const result = findCheapestWindow([], 60);
    expect(result).toBeNull();
  });

  it("returns null for zero duration", () => {
    const prices = [makeTotalPrice(0, 5)];
    const result = findCheapestWindow(prices, 0);
    expect(result).toBeNull();
  });

  it("handles single-hour window", () => {
    const prices = [
      makeTotalPrice(0, 10),
      makeTotalPrice(1, 3),
      makeTotalPrice(2, 8),
    ];

    const result = findCheapestWindow(prices, 60);

    expect(result).not.toBeNull();
    expect(result?.prices).toHaveLength(1);
    expect(result?.averageTotalCentsKwh).toBe(3);
    expect(result?.start).toContain("T01:");
  });

  it("handles window exactly matching all prices", () => {
    const prices = [
      makeTotalPrice(0, 5),
      makeTotalPrice(1, 3),
      makeTotalPrice(2, 7),
    ];

    const result = findCheapestWindow(prices, 180);

    expect(result).not.toBeNull();
    expect(result?.prices).toHaveLength(3);
    expect(result?.averageTotalCentsKwh).toBe(5); // (5+3+7)/3
  });

  it("is provably optimal — brute-force verification", () => {
    // Generate 24 hours of random-ish prices
    const prices: TotalPrice[] = Array.from({ length: 24 }, (_, i) =>
      makeTotalPrice(i, Math.sin(i) * 5 + 8),
    );

    const durationMinutes = 180; // 3 hours
    const windowSize = 3;

    const result = findCheapestWindow(prices, durationMinutes);
    expect(result).not.toBeNull();

    // Brute-force: check every possible 3-hour window
    let bruteForceMin = Infinity;
    for (let i = 0; i <= prices.length - windowSize; i++) {
      let sum = 0;
      for (let j = i; j < i + windowSize; j++) {
        // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
        sum += prices[j]!.totalCentsKwh;
      }
      const avg = sum / windowSize;
      if (avg < bruteForceMin) {
        bruteForceMin = avg;
      }
    }

    const bruteForceMinRounded = Math.round(bruteForceMin * 1000) / 1000;
    expect(result?.averageTotalCentsKwh).toBe(bruteForceMinRounded);
  });

  it("is provably optimal with negative prices", () => {
    const prices: TotalPrice[] = [
      makeTotalPrice(0, 5),
      makeTotalPrice(1, -2),
      makeTotalPrice(2, -3),
      makeTotalPrice(3, 1),
      makeTotalPrice(4, 10),
      makeTotalPrice(5, -1),
      makeTotalPrice(6, -4),
      makeTotalPrice(7, 2),
    ];

    const result = findCheapestWindow(prices, 120); // 2 hours
    expect(result).not.toBeNull();

    // Brute-force verification
    let bruteForceMin = Infinity;
    for (let i = 0; i <= prices.length - 2; i++) {
      // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
      const avg = (prices[i]!.totalCentsKwh + prices[i + 1]!.totalCentsKwh) / 2;
      if (avg < bruteForceMin) {
        bruteForceMin = avg;
      }
    }

    const bruteForceMinRounded = Math.round(bruteForceMin * 1000) / 1000;
    expect(result?.averageTotalCentsKwh).toBe(bruteForceMinRounded);
  });
});

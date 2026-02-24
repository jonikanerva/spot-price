import { describe, it, expect } from "vitest";
import { eurMwhToCentsKwh } from "./nordpool.js";

describe("eurMwhToCentsKwh", () => {
  it("converts 110.5 EUR/MWh to 11.05 c/kWh", () => {
    expect(eurMwhToCentsKwh(110.5)).toBe(11.05);
  });

  it("converts 0 EUR/MWh to 0 c/kWh", () => {
    expect(eurMwhToCentsKwh(0)).toBe(0);
  });

  it("converts negative prices correctly", () => {
    expect(eurMwhToCentsKwh(-15.3)).toBe(-1.53);
  });

  it("rounds to 3 decimal places", () => {
    // 45.237 / 10 = 4.5237 → rounds to 4.524
    expect(eurMwhToCentsKwh(45.237)).toBe(4.524);
  });
});

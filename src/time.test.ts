import { describe, it, expect } from "vitest";
import { getUtcRangeForLocalDate } from "./time.js";

describe("getUtcRangeForLocalDate", () => {
  it("converts Helsinki date to correct UTC range (UTC+2 winter)", () => {
    // Helsinki is UTC+2 in winter (EET)
    // 2026-02-25 00:00 Helsinki = 2026-02-24 22:00 UTC
    // 2026-02-26 00:00 Helsinki = 2026-02-25 22:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-02-25",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-02-24T22:00:00.000Z");
    expect(endUtc).toBe("2026-02-25T22:00:00.000Z");
  });

  it("converts Berlin date to correct UTC range (UTC+1 winter)", () => {
    // Berlin is UTC+1 in winter (CET)
    // 2026-02-25 00:00 Berlin = 2026-02-24 23:00 UTC
    // 2026-02-26 00:00 Berlin = 2026-02-25 23:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-02-25",
      "Europe/Berlin",
    );

    expect(startUtc).toBe("2026-02-24T23:00:00.000Z");
    expect(endUtc).toBe("2026-02-25T23:00:00.000Z");
  });

  it("converts UTC date to same-day range", () => {
    // UTC+0: midnight UTC = midnight UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate("2026-02-25", "UTC");

    expect(startUtc).toBe("2026-02-25T00:00:00.000Z");
    expect(endUtc).toBe("2026-02-26T00:00:00.000Z");
  });

  it("handles summer time (UTC+3 for Helsinki)", () => {
    // Helsinki is UTC+3 in summer (EEST)
    // 2026-07-15 00:00 Helsinki = 2026-07-14 21:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-07-15",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-07-14T21:00:00.000Z");
    expect(endUtc).toBe("2026-07-15T21:00:00.000Z");
  });
});

import { describe, it, expect } from "vitest";
import {
  getUtcRangeForLocalDate,
  getUtcRangeForLocalDateSpan,
} from "./time.js";

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

  it("handles DST spring forward (Helsinki 2026-03-29, clocks +1h at 03:00)", () => {
    // On DST transition day, midnight Helsinki is still EET (UTC+2)
    // 2026-03-29 00:00 Helsinki = 2026-03-28 22:00 UTC (EET, before switch)
    // The local day is 23 hours long, but the UTC range is still 24h
    // because the offset at midnight is +02:00
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-03-29",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-03-28T22:00:00.000Z");
    expect(endUtc).toBe("2026-03-29T22:00:00.000Z");
  });

  it("handles DST fall back (Helsinki 2026-10-25, clocks -1h at 04:00)", () => {
    // On fall back day, midnight Helsinki is EEST (UTC+3)
    // 2026-10-25 00:00 Helsinki = 2026-10-24 21:00 UTC (EEST, before switch)
    // The local day is 25 hours long, but the UTC range is still 24h
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-10-25",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-10-24T21:00:00.000Z");
    expect(endUtc).toBe("2026-10-25T21:00:00.000Z");
  });

  it("handles DST spring forward for Berlin (2026-03-29)", () => {
    // Berlin midnight on spring forward day is CET (UTC+1)
    // 2026-03-29 00:00 Berlin = 2026-03-28 23:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-03-29",
      "Europe/Berlin",
    );

    expect(startUtc).toBe("2026-03-28T23:00:00.000Z");
    expect(endUtc).toBe("2026-03-29T23:00:00.000Z");
  });
});

describe("getUtcRangeForLocalDateSpan", () => {
  it("spans a multi-day Helsinki winter range (UTC+2 throughout)", () => {
    // 2026-02-01 .. 2026-02-05 inclusive, all EET (UTC+2)
    // start = 2026-02-01 00:00 Helsinki = 2026-01-31 22:00 UTC
    // end   = 2026-02-05 end           = 2026-02-05 22:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDateSpan(
      "2026-02-01",
      "2026-02-05",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-01-31T22:00:00.000Z");
    expect(endUtc).toBe("2026-02-05T22:00:00.000Z");
  });

  it("resolves each endpoint's own offset across a spring-forward DST span", () => {
    // Span from before the 2026-03-29 spring-forward to after it.
    // from = 2026-03-20 (EET, +02:00): midnight = 2026-03-19 22:00 UTC
    // to   = 2026-04-05 (EEST, +03:00): span end = 2026-04-06 midnight = 2026-04-05 21:00 UTC
    // The +2 start offset and +3 end offset prove the span is not computed
    // from a single shared offset.
    const span = getUtcRangeForLocalDateSpan(
      "2026-03-20",
      "2026-04-05",
      "Europe/Helsinki",
    );

    expect(span.startUtc).toBe(
      getUtcRangeForLocalDate("2026-03-20", "Europe/Helsinki").startUtc,
    );
    // End is the local midnight that STARTS the day after `to` (2026-04-06),
    // not `to`'s hard +24h endUtc. For this normal 24h to-date they coincide.
    expect(span.endUtc).toBe(
      getUtcRangeForLocalDate("2026-04-06", "Europe/Helsinki").startUtc,
    );
    // Explicit offsets: start at -02:00 (22:00 prev day), end at -03:00 (21:00).
    expect(span.startUtc).toBe("2026-03-19T22:00:00.000Z");
    expect(span.endUtc).toBe("2026-04-05T21:00:00.000Z");
  });

  it("single-day span (from === to) ends at the next day's local midnight", () => {
    const single = getUtcRangeForLocalDateSpan(
      "2026-02-25",
      "2026-02-25",
      "Europe/Helsinki",
    );
    const day = getUtcRangeForLocalDate("2026-02-25", "Europe/Helsinki");
    const nextDay = getUtcRangeForLocalDate("2026-02-26", "Europe/Helsinki");

    expect(single.startUtc).toBe(day.startUtc);
    // Span end is the next day's midnight start; for this normal 24h day that
    // equals the single-date endUtc, but the span no longer defers to it.
    expect(single.endUtc).toBe(nextDay.startUtc);
  });

  it("is not Helsinki-hard-coded — works for Europe/Oslo (UTC+1 winter)", () => {
    // Oslo winter is CET (UTC+1), same as Berlin.
    // from = 2026-02-01 midnight Oslo = 2026-01-31 23:00 UTC
    // to   = 2026-02-03 end           = 2026-02-03 23:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDateSpan(
      "2026-02-01",
      "2026-02-03",
      "Europe/Oslo",
    );

    expect(startUtc).toBe("2026-01-31T23:00:00.000Z");
    expect(endUtc).toBe("2026-02-03T23:00:00.000Z");
  });

  it("keeps the 25th hour of a fall-back to-date (span crossing the transition)", () => {
    // Helsinki fall-back is 2026-10-25 (clocks 04:00 EEST -> 03:00 EET), a 25h
    // local day. Span 2026-10-24 .. 2026-10-25:
    // start = 2026-10-24 00:00 (EEST, +03:00) = 2026-10-23 21:00 UTC
    // end   = 2026-10-26 00:00 (EET,  +02:00) = 2026-10-25 22:00 UTC  (49h span)
    // The old +24h-of-to-date endUtc was 2026-10-25 21:00 UTC, dropping the 25th hour.
    const { startUtc, endUtc } = getUtcRangeForLocalDateSpan(
      "2026-10-24",
      "2026-10-25",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-10-23T21:00:00.000Z");
    expect(endUtc).toBe("2026-10-25T22:00:00.000Z");
    const spanHours =
      (new Date(endUtc).getTime() - new Date(startUtc).getTime()) / 3_600_000;
    expect(spanHours).toBe(49);
  });

  it("a single fall-back day span is 25h, not the old buggy 24h", () => {
    // from === to === 2026-10-25 (the 25h fall-back day):
    // start = 2026-10-25 00:00 (EEST, +03:00) = 2026-10-24 21:00 UTC
    // end   = 2026-10-26 00:00 (EET,  +02:00) = 2026-10-25 22:00 UTC  (25h)
    // Before Option A the end was 2026-10-25 21:00 UTC (24h) — one hour short.
    const { startUtc, endUtc } = getUtcRangeForLocalDateSpan(
      "2026-10-25",
      "2026-10-25",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-10-24T21:00:00.000Z");
    expect(endUtc).toBe("2026-10-25T22:00:00.000Z");
    const spanHours =
      (new Date(endUtc).getTime() - new Date(startUtc).getTime()) / 3_600_000;
    expect(spanHours).toBe(25);
  });
});

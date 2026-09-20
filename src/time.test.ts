import { afterEach, describe, it, expect, vi } from "vitest";
import {
  getCurrentAndNextDate,
  getUtcRangeForLocalDate,
  getUtcRangeForLocalDateSpan,
} from "./time.js";

/** Length of a UTC range in hours — 23, 24 or 25 for one local calendar day. */
const rangeHours = (startUtc: string, endUtc: string): number =>
  (new Date(endUtc).getTime() - new Date(startUtc).getTime()) / 3_600_000;

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
    // The local day 2026-03-29 is 23 hours long, and the UTC range is 23 hours
    // to match. Each edge resolves its own offset:
    //   start = 2026-03-29 00:00 Helsinki (EET, +02:00) = 2026-03-28 22:00 UTC
    //   end   = 2026-03-30 00:00 Helsinki (EEST, +03:00) = 2026-03-29 21:00 UTC
    // A fixed +24h end would run to 2026-03-29 22:00 UTC and overshoot the day
    // by one hour, pulling in the first interval of 2026-03-30.
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-03-29",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-03-28T22:00:00.000Z");
    expect(endUtc).toBe("2026-03-29T21:00:00.000Z");
    expect(rangeHours(startUtc, endUtc)).toBe(23);
  });

  it("handles DST fall back (Helsinki 2026-10-25, clocks -1h at 04:00)", () => {
    // The local day 2026-10-25 is 25 hours long, and the UTC range is 25 hours
    // to match. Each edge resolves its own offset:
    //   start = 2026-10-25 00:00 Helsinki (EEST, +03:00) = 2026-10-24 21:00 UTC
    //   end   = 2026-10-26 00:00 Helsinki (EET,  +02:00) = 2026-10-25 22:00 UTC
    // A fixed +24h end would stop at 2026-10-25 21:00 UTC and drop the 25th
    // hour — the four quarter-hours Nord Pool publishes as local 23:00–24:00.
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-10-25",
      "Europe/Helsinki",
    );

    expect(startUtc).toBe("2026-10-24T21:00:00.000Z");
    expect(endUtc).toBe("2026-10-25T22:00:00.000Z");
    expect(rangeHours(startUtc, endUtc)).toBe(25);
  });

  it("handles DST spring forward for Berlin (2026-03-29)", () => {
    // Not Helsinki-hard-coded: Berlin runs CET (+01:00) into the transition and
    // CEST (+02:00) out of it, so the same 23-hour local day shows up shifted.
    //   start = 2026-03-29 00:00 Berlin (CET,  +01:00) = 2026-03-28 23:00 UTC
    //   end   = 2026-03-30 00:00 Berlin (CEST, +02:00) = 2026-03-29 22:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-03-29",
      "Europe/Berlin",
    );

    expect(startUtc).toBe("2026-03-28T23:00:00.000Z");
    expect(endUtc).toBe("2026-03-29T22:00:00.000Z");
    expect(rangeHours(startUtc, endUtc)).toBe(23);
  });

  it("handles DST fall back for Berlin (2026-10-25)", () => {
    //   start = 2026-10-25 00:00 Berlin (CEST, +02:00) = 2026-10-24 22:00 UTC
    //   end   = 2026-10-26 00:00 Berlin (CET,  +01:00) = 2026-10-25 23:00 UTC
    const { startUtc, endUtc } = getUtcRangeForLocalDate(
      "2026-10-25",
      "Europe/Berlin",
    );

    expect(startUtc).toBe("2026-10-24T22:00:00.000Z");
    expect(endUtc).toBe("2026-10-25T23:00:00.000Z");
    expect(rangeHours(startUtc, endUtc)).toBe(25);
  });

  it("keeps a normal day at 24 hours in every supported timezone", () => {
    for (const timeZone of ["Europe/Helsinki", "Europe/Berlin", "UTC"]) {
      const { startUtc, endUtc } = getUtcRangeForLocalDate(
        "2026-07-15",
        timeZone,
      );
      expect(rangeHours(startUtc, endUtc)).toBe(24);
    }
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
    // End is the local midnight that STARTS the day after `to` (2026-04-06).
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
    // A one-day span and the single-day range are the same range, because both
    // are built from `startOfLocalDayUtc` at both edges.
    expect(single.endUtc).toBe(nextDay.startUtc);
    expect(single.endUtc).toBe(day.endUtc);
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
    // A 24h+24h span would end at 2026-10-25 21:00 UTC and drop the 25th hour.
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
    // A fixed 24h span would end at 2026-10-25 21:00 UTC — one hour short.
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

describe("getCurrentAndNextDate", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Pin the clock to a UTC instant. Only `Date` is faked — no timers run here. */
  const atUtc = (isoInstant: string): void => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(isoInstant));
  };

  it("returns consecutive labels on a normal day", () => {
    atUtc("2026-02-25T12:00:00Z");

    expect(getCurrentAndNextDate("Europe/Helsinki")).toEqual({
      today: "2026-02-25",
      tomorrow: "2026-02-26",
    });
  });

  it("rolls over month and year boundaries", () => {
    // Helsinki is EET (+02:00) in winter, so this instant is local 2027-01-01.
    atUtc("2026-12-31T22:30:00Z");

    expect(getCurrentAndNextDate("Europe/Helsinki")).toEqual({
      today: "2027-01-01",
      tomorrow: "2027-01-02",
    });
  });

  it("does not collide on the fall-back day (Helsinki 2026-10-25, 25h local)", () => {
    // Local 2026-10-25 00:30, still EEST (+03:00). The local date lasts 25
    // hours, so a 24h shift of this instant stays inside it and yields
    // tomorrow === today. Calendar arithmetic on the label cannot.
    atUtc("2026-10-24T21:30:00Z");

    const { today, tomorrow } = getCurrentAndNextDate("Europe/Helsinki");
    expect(today).toBe("2026-10-25");
    expect(tomorrow).toBe("2026-10-26");
    expect(tomorrow).not.toBe(today);
  });

  it("stays correct after the fall-back transition (Helsinki 2026-10-25)", () => {
    // Local 2026-10-25 12:00, now EET (+02:00) — the other side of the switch.
    atUtc("2026-10-25T10:00:00Z");

    expect(getCurrentAndNextDate("Europe/Helsinki")).toEqual({
      today: "2026-10-25",
      tomorrow: "2026-10-26",
    });
  });

  it("does not skip the spring-forward day (Helsinki 2026-03-29, 23h local)", () => {
    // Local 2026-03-28 23:30, EET (+02:00). The next local date lasts only 23
    // hours, so a 24h shift of this instant jumps clean over it and yields
    // 2026-03-30 — the whole date 2026-03-29 disappears.
    atUtc("2026-03-28T21:30:00Z");

    const { today, tomorrow } = getCurrentAndNextDate("Europe/Helsinki");
    expect(today).toBe("2026-03-28");
    expect(tomorrow).toBe("2026-03-29");
  });

  it("stays correct inside the spring-forward day (Helsinki 2026-03-29)", () => {
    // Local 2026-03-29 00:30, before the 03:00 switch.
    atUtc("2026-03-28T22:30:00Z");

    expect(getCurrentAndNextDate("Europe/Helsinki")).toEqual({
      today: "2026-03-29",
      tomorrow: "2026-03-30",
    });
  });

  it("is not Helsinki-hard-coded — Berlin fall-back day (2026-10-25)", () => {
    // Local 2026-10-25 00:30 in Berlin, still CEST (+02:00).
    atUtc("2026-10-24T22:30:00Z");

    expect(getCurrentAndNextDate("Europe/Berlin")).toEqual({
      today: "2026-10-25",
      tomorrow: "2026-10-26",
    });
  });
});

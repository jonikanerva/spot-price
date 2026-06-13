import { afterEach, describe, it, expect, vi } from "vitest";
import { eurMwhToCentsKwh, fetchDayAheadPrices } from "./nordpool.js";

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

const jsonResponse = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

/** Raw (non-stringified) body, for the invalid-JSON case. */
const rawResponse = (text: string, status = 200): Response =>
  new Response(text, {
    status,
    headers: { "Content-Type": "application/json" },
  });

const params = { date: "2026-06-09", areas: ["FI"] as const };

/**
 * Drive a promise that triggers two `await delay(7000)` retries to completion
 * under fake timers. We advance time after the in-flight microtasks settle so
 * each `setTimeout` fires; awaiting the result then resolves once the loop is
 * exhausted.
 */
const runWithRetries = async <T>(start: () => Promise<T>): Promise<T> => {
  const promise = start();
  // Two retry waits (attempts 1 and 2) of RETRY_DELAY_MS each.
  await vi.advanceTimersByTimeAsync(7000);
  await vi.advanceTimersByTimeAsync(7000);
  return promise;
};

describe("fetchDayAheadPrices", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("parses a valid body into prices without retrying", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { FI: 42.5 },
          },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([
      {
        deliveryStart: "2026-06-09T00:00:00Z",
        deliveryEnd: "2026-06-09T01:00:00Z",
        priceEurMwh: 42.5,
        area: "FI",
      },
    ]);
  });

  it("ignores additive cosmetic envelope and unknown entry fields (Obj 1)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        deliveryDateCET: "2026-06-09",
        updatedAt: "2026-06-08T11:00:00Z",
        currency: "EUR",
        areaStates: [{ state: "Final", areas: ["FI"] }],
        somethingBrandNew: { nested: true },
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { FI: 50 },
            futureField: "ignored",
          },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toHaveLength(1);
    expect(result[0]?.priceEurMwh).toBe(50);
  });

  it("skips one malformed entry and keeps the good FI price (Obj 4)", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { FI: 31.2 },
          },
          { entryPerArea: "broken" },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([
      {
        deliveryStart: "2026-06-09T00:00:00Z",
        deliveryEnd: "2026-06-09T01:00:00Z",
        priceEurMwh: 31.2,
        area: "FI",
      },
    ]);
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("degrades to [] with NORDPOOL_SCHEMA_DRIFT on envelope drift (Obj 2+3)", async () => {
    vi.useFakeTimers();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
    // A Response body is single-use; the retry loop fetches MAX_RETRIES times,
    // so each attempt must receive a fresh Response (as a real fetch would).
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(jsonResponse({ unexpected: "shape" })),
    );

    const result = await runWithRetries(() => fetchDayAheadPrices(params));
    expect(result).toEqual([]);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(errorSpy.mock.calls[0]?.[0]).toContain("NORDPOOL_SCHEMA_DRIFT");

    vi.useRealTimers();
  });

  it("degrades to [] with NORDPOOL_SCHEMA_DRIFT on invalid JSON (SyntaxError path)", async () => {
    vi.useFakeTimers();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
    // Fresh Response per attempt: the SyntaxError path also exhausts the retry
    // budget, and each retry re-reads the body.
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(rawResponse("this is not json")),
    );

    const result = await runWithRetries(() => fetchDayAheadPrices(params));
    expect(result).toEqual([]);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(errorSpy.mock.calls[0]?.[0]).toContain("NORDPOOL_SCHEMA_DRIFT");

    vi.useRealTimers();
  });

  it("returns [] immediately for an empty body and does not log drift", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(globalThis, "fetch").mockResolvedValue(rawResponse(""));

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([]);
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("returns [] when the requested area key is absent, without throwing", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { SE1: 50 },
          },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([]);
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("skips an entry whose price is the wrong type, yielding [] without throwing", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { FI: "x" },
          },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([]);
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("keeps the FI price when a sibling area is null (Obj 5)", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { FI: 31.2, SE1: null },
          },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([
      {
        deliveryStart: "2026-06-09T00:00:00Z",
        deliveryEnd: "2026-06-09T01:00:00Z",
        priceEurMwh: 31.2,
        area: "FI",
      },
    ]);
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("skips a null FI value, yielding [] without drift log (Obj 5)", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        multiAreaEntries: [
          {
            deliveryStart: "2026-06-09T00:00:00Z",
            deliveryEnd: "2026-06-09T01:00:00Z",
            entryPerArea: { FI: null },
          },
        ],
      }),
    );

    const result = await fetchDayAheadPrices(params);
    expect(result).toEqual([]);
    expect(errorSpy).not.toHaveBeenCalled();
  });
});

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  dailyToRecords,
  fetchWeather,
  HELSINKI,
  hourlyToRecords,
} from "./weather.js";

const jsonResponse = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const POINT = HELSINKI;
// 13:37 UTC — must be truncated to the 13:00 issuance hour when stored.
const ISSUED_AT = new Date("2026-06-13T13:37:42.000Z");

// dt is a UNIX epoch in SECONDS; 1781442000 -> 2026-06-14T13:00:00Z.
const VALID_HOURLY = {
  dt: 1781442000,
  temp: 11.46,
  clouds: 100,
  uvi: 0.12,
  wind_speed: 5.23,
  wind_deg: 339,
  // Extra OWM fields the schema must tolerate (not .strict()).
  feels_like: 10.9,
  pressure: 1011,
  humidity: 87,
  pop: 0.4,
};

const fetchParams = { apiKey: "test-key", point: POINT, issuedAt: ISSUED_AT };

/** UNIX epoch SECONDS of a UTC ISO instant, so the fixtures stay readable. */
const epoch = (iso: string): number => Math.floor(Date.parse(iso) / 1000);

// One Call sets `daily[].dt` to LOCAL NOON at the point; what we store is the
// UTC calendar date of that instant.
const VALID_DAILY = {
  dt: epoch("2026-06-14T09:00:00.000Z"),
  sunrise: epoch("2026-06-14T01:03:00.000Z"),
  sunset: epoch("2026-06-14T19:48:00.000Z"),
  temp: { morn: 9.5, day: 15.2, eve: 13.1, night: 10.4, min: 8.8, max: 16.3 },
  clouds: 42,
  uvi: 4.2,
  // Extra OWM fields the schema must tolerate (not .strict()).
  feels_like: { day: 14.8, night: 9.9, eve: 12.7, morn: 8.9 },
  pressure: 1011,
  humidity: 60,
  pop: 0.2,
  wind_speed: 4.1,
  wind_deg: 200,
  summary: "Expect a day of partly cloudy weather",
  moonrise: epoch("2026-06-14T02:10:00.000Z"),
  moon_phase: 0.75,
};

describe("hourlyToRecords (pure)", () => {
  it("maps dt seconds to a UTC targetTime, truncates issuedAt to the hour, and passes fields through", () => {
    const records = hourlyToRecords(POINT, ISSUED_AT, {
      hourly: [VALID_HOURLY],
    });

    expect(records).toHaveLength(1);
    const r = records[0];
    expect(r?.pointId).toBe(POINT.id);
    expect(r?.issuedAt).toBe("2026-06-13T13:00:00.000Z");
    expect(r?.targetTime).toBe("2026-06-14T13:00:00.000Z");
    expect(r?.temp).toBe(11.46);
    expect(r?.clouds).toBe(100);
    expect(r?.uvi).toBe(0.12);
    expect(r?.windSpeed).toBe(5.23);
    expect(r?.windDeg).toBe(339);
  });
});

describe("dailyToRecords (pure)", () => {
  it("maps the six temps, the daily scalars and the solar bounds, and derives the UTC calendar date", () => {
    const records = dailyToRecords(POINT, ISSUED_AT, { daily: [VALID_DAILY] });

    expect(records).toHaveLength(1);
    const r = records[0];
    expect(r?.pointId).toBe(POINT.id);
    expect(r?.issuedAt).toBe("2026-06-13T13:00:00.000Z");
    expect(r?.targetDate).toBe("2026-06-14");
    // The raw instant is kept as provenance, so the date stays recomputable.
    expect(r?.targetDt).toBe("2026-06-14T09:00:00.000Z");
    expect(r?.tempMorn).toBe(9.5);
    expect(r?.tempDay).toBe(15.2);
    expect(r?.tempEve).toBe(13.1);
    expect(r?.tempNight).toBe(10.4);
    expect(r?.tempMin).toBe(8.8);
    expect(r?.tempMax).toBe(16.3);
    expect(r?.clouds).toBe(42);
    expect(r?.uvi).toBe(4.2);
    expect(r?.sunrise).toBe("2026-06-14T01:03:00.000Z");
    expect(r?.sunset).toBe("2026-06-14T19:48:00.000Z");
  });

  it("derives target_date on the UTC day boundary, never a local one", () => {
    // Last instant of one UTC day and the first of the next. A local-time
    // derivation (STACK.md §7 forbids it below the response boundary) would put
    // these on the same day for a point east of UTC.
    const lastOfDay = { ...VALID_DAILY, dt: epoch("2026-06-14T23:59:59.000Z") };
    const firstOfNext = {
      ...VALID_DAILY,
      dt: epoch("2026-06-15T00:00:00.000Z"),
    };

    const records = dailyToRecords(POINT, ISSUED_AT, {
      daily: [lastOfDay, firstOfNext],
    });
    expect(records.map((r) => r.targetDate)).toEqual([
      "2026-06-14",
      "2026-06-15",
    ]);
  });

  it("keeps sunrise/sunset null when One Call omits them (polar day / polar night)", () => {
    const { sunrise, sunset, ...polar } = VALID_DAILY;
    expect(sunrise).toBeDefined();
    expect(sunset).toBeDefined();

    const records = dailyToRecords(POINT, ISSUED_AT, { daily: [polar] });
    expect(records).toHaveLength(1);
    expect(records[0]?.sunrise).toBeNull();
    expect(records[0]?.sunset).toBeNull();
    // The rest of the row is unaffected.
    expect(records[0]?.uvi).toBe(4.2);
  });
});

describe("fetchWeather", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("parses a valid response into records with correct UTC targetTime", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ hourly: [VALID_HOURLY] }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.records[0]?.targetTime).toBe("2026-06-14T13:00:00.000Z");
    expect(result.records[0]?.issuedAt).toBe("2026-06-13T13:00:00.000Z");
    expect(result.records[0]?.windSpeed).toBe(5.23);
  });

  it("degrades (does not throw) on 401 auth failure", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({}, 401));
    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    expect(result.records).toHaveLength(0);
    if (!result.ok) {
      expect(result.reason).toContain("auth");
    }
  });

  it("degrades on 403 auth failure", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({}, 403));
    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("auth");
    }
  });

  it("degrades on a non-OK HTTP status", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({}, 500));
    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("500");
    }
  });

  it("degrades on a malformed body that fails schema validation", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ hourly: [{ dt: "not-a-number" }] }),
    );
    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("schema");
    }
  });

  it("degrades on a thrown network error", async () => {
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new Error("ECONNRESET"));
    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("ECONNRESET");
    }
  });

  it("degrades on an aborted (timed-out) request", async () => {
    const abortError = new Error("aborted");
    abortError.name = "AbortError";
    vi.spyOn(globalThis, "fetch").mockRejectedValue(abortError);
    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("timed out");
    }
  });

  it("asks for the daily block in the same call — `exclude` drops it no more, and the call count is unchanged", async () => {
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(
        jsonResponse({ hourly: [VALID_HOURLY], daily: [VALID_DAILY] }),
      );

    await fetchWeather(fetchParams);

    // ONE request, not two: the subscription is billed per call, so the daily
    // block is free and must never become a second call.
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [requestedUrl] = fetchSpy.mock.calls[0] ?? [];
    // Runtime guard rather than a cast (STACK.md §7 forbids `as` here).
    if (typeof requestedUrl !== "string") {
      throw new Error("expected fetch to be called with a URL string");
    }
    const requested = new URL(requestedUrl);
    expect(requested.searchParams.get("exclude")).toBe(
      "current,minutely,alerts",
    );
  });

  it("parses the daily block from the same response", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ hourly: [VALID_HOURLY], daily: [VALID_DAILY] }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.daily.ok).toBe(true);
    expect(result.daily.records).toHaveLength(1);
    expect(result.daily.records[0]?.targetDate).toBe("2026-06-14");
  });

  it("ISOLATION: a malformed daily block still yields every hourly record", async () => {
    // One shared schema would fail the whole parse here and DISCARD the hourly
    // rows — and that issuance can never be re-fetched once its hour has passed.
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        hourly: [VALID_HOURLY],
        daily: [{ dt: "not-a-number", temp: "nonsense" }],
      }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.records[0]?.targetTime).toBe("2026-06-14T13:00:00.000Z");
    // …and the daily failure is reported, not swallowed.
    expect(result.daily.ok).toBe(false);
    if (!result.daily.ok) {
      expect(result.daily.reason).toContain("daily");
    }
  });

  it("ISOLATION: a missing daily block still yields every hourly record", async () => {
    // The regression guard: if the `exclude` change ever stops taking effect in
    // production, the daily result degrades with a reason while the hourly
    // collection — the asset that already exists — keeps running untouched.
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ hourly: [VALID_HOURLY] }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.daily.ok).toBe(false);
    expect(result.daily.records).toHaveLength(0);
  });

  it("ISOLATION: a malformed hourly block does not discard a valid daily block", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ hourly: [{ dt: "not-a-number" }], daily: [VALID_DAILY] }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(false);
    expect(result.records).toHaveLength(0);
    expect(result.daily.ok).toBe(true);
    expect(result.daily.records).toHaveLength(1);
  });

  it("ISOLATION: an out-of-range epoch in the daily block still yields every hourly record", async () => {
    // `z.number()` rejects NaN and Infinity but accepts 1e13, and 1e13 seconds
    // is past the Date range, so the mapping would throw `RangeError` inside the
    // SHARED try of `fetchWeather` — degrading the hourly result and discarding
    // rows that parsed fine. The bound in the schema keeps it a daily-only
    // degrade.
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        hourly: [VALID_HOURLY],
        daily: [{ ...VALID_DAILY, dt: 1e13 }],
      }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.daily.ok).toBe(false);
    expect(result.daily.records).toHaveLength(0);
    if (!result.daily.ok) {
      // The SCHEMA bound is what rejected it, not the mapping try/catch. Pinning
      // the reason keeps the two mechanisms separable: without this assertion a
      // later cleanup could delete the epoch bound as dead code and the suite
      // would stay green, because the try/catch alone also keeps the hourly rows
      // (it would report "failed mapping: Invalid time value" instead).
      expect(result.daily.reason).toContain("schema validation");
      expect(result.daily.reason).not.toContain("mapping");
    }
  });

  it("ISOLATION: an out-of-range sunrise in the daily block still yields every hourly record", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        hourly: [VALID_HOURLY],
        daily: [{ ...VALID_DAILY, sunrise: -1e13 }],
      }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.daily.ok).toBe(false);
  });

  it("tolerates unknown daily fields and any array length (not .strict())", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        hourly: [VALID_HOURLY],
        // Seven entries is the documented period; the schema must not depend on
        // it, and `summary`/`moon_phase`/`pop` are extra fields it must ignore.
        daily: Array.from({ length: 8 }, (_, i) => ({
          ...VALID_DAILY,
          dt: epoch("2026-06-14T09:00:00.000Z") + i * 86_400,
          brand_new_owm_field: "whatever",
        })),
      }),
    );

    const result = await fetchWeather(fetchParams);
    expect(result.daily.ok).toBe(true);
    expect(result.daily.records).toHaveLength(8);
    expect(result.daily.records[7]?.targetDate).toBe("2026-06-21");
  });
});

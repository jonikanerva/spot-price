import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchWeather, HELSINKI, hourlyToRecords } from "./weather.js";

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
});

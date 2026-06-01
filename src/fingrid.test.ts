import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchFingridSeries } from "./fingrid.js";

const jsonResponse = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const range = {
  apiKey: "test-key",
  startUtc: "2026-03-01T00:00:00.000Z",
  endUtc: "2026-03-02T00:00:00.000Z",
};

describe("fetchFingridSeries", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("parses a valid response into records", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        data: [
          {
            datasetId: 245,
            startTime: "2026-03-01T00:00:00.000Z",
            endTime: "2026-03-01T00:15:00.000Z",
            value: 4444.7,
          },
        ],
        pagination: { total: 1 },
      }),
    );

    const result = await fetchFingridSeries(range);
    expect(result.ok).toBe(true);
    expect(result.records).toHaveLength(1);
    expect(result.records[0]?.datasetId).toBe(245);
  });

  it("degrades (does not throw) on 401/403 auth failure", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({}, 401));
    const result = await fetchFingridSeries(range);
    expect(result.ok).toBe(false);
    expect(result.records).toHaveLength(0);
    if (!result.ok) {
      expect(result.reason).toContain("auth");
    }
  });

  it("degrades on HTTP 422", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({}, 422));
    const result = await fetchFingridSeries(range);
    expect(result.ok).toBe(false);
  });

  it("degrades on a malformed body that fails schema validation", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({ data: [{ datasetId: "not-a-number" }] }),
    );
    const result = await fetchFingridSeries(range);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("schema");
    }
  });

  it("degrades on a thrown network error", async () => {
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new Error("ECONNRESET"));
    const result = await fetchFingridSeries(range);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("ECONNRESET");
    }
  });

  it("degrades on an aborted (timed-out) request", async () => {
    const abortError = new Error("aborted");
    abortError.name = "AbortError";
    vi.spyOn(globalThis, "fetch").mockRejectedValue(abortError);
    const result = await fetchFingridSeries(range);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.reason).toContain("timed out");
    }
  });
});

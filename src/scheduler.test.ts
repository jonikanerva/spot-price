import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import type cron from "node-cron";

vi.mock("node-cron", () => ({
  default: {
    schedule: vi.fn(() => ({ stop: vi.fn() })),
  },
}));

vi.mock("./fetch-job.js", () => ({
  runFetchJob: vi.fn(),
}));

type ScheduleMock = ReturnType<typeof vi.mocked<typeof cron.schedule>>;

/** Extract a cron callback by call index — throws if missing. */
const getCronCallback = (mock: ScheduleMock, index: number): (() => void) => {
  const call = mock.mock.calls[index];
  if (!call) {
    throw new Error(`Expected cron.schedule call at index ${String(index)}`);
  }
  return call[1] as () => void;
};

/** Flush fire-and-forget async chains. */
const flushAsync = (): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, 50));

describe("scheduler", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it("schedules standard and burst cron jobs", async () => {
    const cronMod = await import("node-cron");
    const { startScheduler } = await import("./scheduler.js");

    const db = {} as Parameters<typeof startScheduler>[0];
    startScheduler(db);

    expect(cronMod.default.schedule).toHaveBeenCalledTimes(2);
    expect(cronMod.default.schedule).toHaveBeenCalledWith(
      "0 */2 * * *",
      expect.any(Function),
    );
    expect(cronMod.default.schedule).toHaveBeenCalledWith(
      "*/10 12-13 * * *",
      expect.any(Function),
      { timezone: "Europe/Oslo" },
    );
  });

  it("stop() calls stop on all scheduled tasks", async () => {
    const cronMod = await import("node-cron");
    const stops = [vi.fn(), vi.fn()];
    vi.mocked(cronMod.default.schedule)
      .mockReturnValueOnce({ stop: stops[0] } as never)
      .mockReturnValueOnce({ stop: stops[1] } as never);

    const { startScheduler } = await import("./scheduler.js");
    const db = {} as Parameters<typeof startScheduler>[0];
    const handle = startScheduler(db);
    handle.stop();

    for (const stop of stops) {
      expect(stop).toHaveBeenCalledOnce();
    }
  });

  it("startup fetch calls runFetchJob immediately", async () => {
    const fetchJob = await import("./fetch-job.js");
    vi.mocked(fetchJob.runFetchJob).mockResolvedValue({
      results: [],
      tomorrowAvailable: false,
    });

    const { runStartupFetch } = await import("./scheduler.js");
    const db = {} as Parameters<typeof runStartupFetch>[0];
    runStartupFetch(db);

    expect(fetchJob.runFetchJob).toHaveBeenCalledWith(db);
  });

  it("startup fetch does not throw when runFetchJob fails", async () => {
    const fetchJob = await import("./fetch-job.js");
    vi.mocked(fetchJob.runFetchJob).mockRejectedValue(
      new Error("DB connection lost"),
    );

    const { runStartupFetch } = await import("./scheduler.js");
    const db = {} as Parameters<typeof runStartupFetch>[0];

    expect(() => runStartupFetch(db)).not.toThrow();
  });

  it("burst callback stops fetching after tomorrow captured", async () => {
    const cronMod = await import("node-cron");
    const fetchJob = await import("./fetch-job.js");
    const { startScheduler } = await import("./scheduler.js");

    vi.mocked(fetchJob.runFetchJob).mockResolvedValue({
      results: [{ date: "2026-04-15", stored: 100, skipped: false }],
      tomorrowAvailable: true,
    });

    const db = {} as Parameters<typeof startScheduler>[0];
    startScheduler(db);

    const burstCallback = getCronCallback(
      vi.mocked(cronMod.default.schedule),
      1,
    );

    // First burst call: fetches and captures tomorrow
    burstCallback();
    await flushAsync();
    expect(fetchJob.runFetchJob).toHaveBeenCalledTimes(1);

    // Second burst: skipped because tomorrow already captured
    burstCallback();
    await flushAsync();
    expect(fetchJob.runFetchJob).toHaveBeenCalledTimes(1);
  });

  it("burst callback keeps fetching when tomorrow not available", async () => {
    const cronMod = await import("node-cron");
    const fetchJob = await import("./fetch-job.js");
    const { startScheduler } = await import("./scheduler.js");

    vi.mocked(fetchJob.runFetchJob).mockResolvedValue({
      results: [{ date: "2026-04-15", stored: 0, skipped: false }],
      tomorrowAvailable: false,
    });

    const db = {} as Parameters<typeof startScheduler>[0];
    startScheduler(db);

    const burstCallback = getCronCallback(
      vi.mocked(cronMod.default.schedule),
      1,
    );

    // First burst: fetches, tomorrow not available
    burstCallback();
    await flushAsync();
    expect(fetchJob.runFetchJob).toHaveBeenCalledTimes(1);

    // Second burst: fetches again since flag not set
    burstCallback();
    await flushAsync();
    expect(fetchJob.runFetchJob).toHaveBeenCalledTimes(2);
  });
});

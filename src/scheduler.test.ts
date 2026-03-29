import { describe, it, expect, vi, afterEach } from "vitest";

vi.mock("node-cron", () => ({
  default: {
    schedule: vi.fn(() => ({ stop: vi.fn() })),
  },
}));

vi.mock("./fetch-job.js", () => ({
  runFetchJob: vi.fn(),
}));

describe("scheduler", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it("schedules cron every 2 hours", async () => {
    const cron = await import("node-cron");
    const { startScheduler } = await import("./scheduler.js");

    const db = {} as Parameters<typeof startScheduler>[0];
    startScheduler(db);

    expect(cron.default.schedule).toHaveBeenCalledWith(
      "0 */2 * * *",
      expect.any(Function),
    );
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
});

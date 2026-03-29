import type { ScheduledTask } from "node-cron";
import cron from "node-cron";
import type { Pool } from "pg";
import { runFetchJob } from "./fetch-job.js";

const safeFetch = async (
  pool: Pool,
  label: string,
): Promise<void> => {
  try {
    const result = await runFetchJob(pool);
    const stored = result.results.reduce((sum, r) => sum + r.stored, 0);
    if (stored > 0) {
      console.log(`[scheduler] ${label}: stored ${String(stored)} new prices`);
    } else {
      console.log(`[scheduler] ${label}: no new data to store`);
    }
    if (!result.tomorrowAvailable) {
      console.log(
        "[scheduler] Tomorrow's prices not yet available — will retry next cycle",
      );
    }
  } catch (error) {
    const msg = error instanceof Error ? error.message : "unknown error";
    console.error(`[scheduler] ${label} failed: ${msg}`);
  }
};

/**
 * Run an immediate fetch on startup so restarts and deploys
 * do not leave gaps until the next scheduled cycle.
 */
export const runStartupFetch = (pool: Pool): void => {
  void safeFetch(pool, "Startup fetch");
};

/**
 * Schedule price fetch every 2 hours.
 *
 * Nord Pool publishes next-day prices at ~12:00 UTC.
 * Instead of a single daily run with complex retry logic, we poll every 2 hours.
 * Each run is idempotent — already-stored data is skipped via allAreasPresent.
 * This naturally handles:
 *   - DST transitions (no publication-time guessing needed)
 *   - Nord Pool outages (next cycle retries automatically)
 *   - Service restarts (startup fetch + next cycle fills gaps)
 */
export const startScheduler = (pool: Pool): ScheduledTask => {
  console.log("[scheduler] Price fetch scheduled every 2 hours");

  return cron.schedule("0 */2 * * *", () => {
    void safeFetch(pool, "Scheduled fetch");
  });
};

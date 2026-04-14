import cron from "node-cron";
import type { Pool } from "pg";
import { runFetchJob } from "./fetch-job.js";
import { formatUtcDate, addDays } from "./time.js";

/**
 * Run a fetch job and return whether tomorrow's data is available.
 * Errors are caught and logged — never throws.
 */
const safeFetch = async (pool: Pool, label: string): Promise<boolean> => {
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
    return result.tomorrowAvailable;
  } catch (error) {
    const msg = error instanceof Error ? error.message : "unknown error";
    console.error(`[scheduler] ${label} failed: ${msg}`);
    return false;
  }
};

/**
 * Run an immediate fetch on startup so restarts and deploys
 * do not leave gaps until the next scheduled cycle.
 */
export const runStartupFetch = (pool: Pool): void => {
  void safeFetch(pool, "Startup fetch");
};

/** Handle for stopping all scheduled tasks. */
export interface SchedulerHandle {
  readonly stop: () => void;
}

/**
 * Schedule price fetching with two strategies:
 *
 * 1. Standard: every 2 hours — reliable baseline and safety net.
 * 2. Burst: every 10 minutes during 12:00–13:59 CET — catches next-day
 *    price publication quickly. Stops once tomorrow's data is found.
 *
 * Nord Pool publishes day-ahead prices at ~12:55 CET (after gate closure
 * at 12:00 CET). Delays can push publication to ~13:45 CET. The burst
 * schedule uses Europe/Oslo timezone so DST is handled automatically.
 *
 * A date-based flag prevents redundant fetches: once tomorrow's prices
 * are captured, remaining burst ticks for that day are skipped.
 * Each run is idempotent — already-stored data is skipped via allAreasPresent.
 */
export const startScheduler = (pool: Pool): SchedulerHandle => {
  let lastCapturedTomorrow: string | null = null;

  console.log(
    "[scheduler] Price fetch scheduled: every 2h + burst every 10min during 12:00–13:59 CET",
  );

  const standard = cron.schedule("0 */2 * * *", () => {
    void safeFetch(pool, "Scheduled fetch");
  });

  const burst = cron.schedule(
    "*/10 12-13 * * *",
    () => {
      const tomorrow = formatUtcDate(addDays(new Date(), 1));
      if (lastCapturedTomorrow === tomorrow) {
        return;
      }
      void (async () => {
        const available = await safeFetch(pool, "Publication-window fetch");
        if (available) {
          lastCapturedTomorrow = tomorrow;
          console.log(
            "[scheduler] Tomorrow's prices captured — burst polling paused for today",
          );
        }
      })();
    },
    { timezone: "Europe/Oslo" },
  );

  return {
    stop: () => {
      void standard.stop();
      void burst.stop();
    },
  };
};

import type { ScheduledTask } from "node-cron";
import cron from "node-cron";
import type Database from "better-sqlite3";
import { runFetchJob } from "./fetch-job.js";

const MAX_RETRY_ATTEMPTS = 8; // 8 retries × 15 min = 2 hours
const RETRY_INTERVAL_MS = 15 * 60 * 1000; // 15 minutes

const retryWithBackoff = async (db: Database.Database): Promise<void> => {
  for (let attempt = 1; attempt <= MAX_RETRY_ATTEMPTS; attempt++) {
    try {
      await runFetchJob(db);
      console.log("[scheduler] Fetch job completed successfully");
      return;
    } catch (error) {
      const msg = error instanceof Error ? error.message : "unknown error";
      console.error(
        `[scheduler] Attempt ${String(attempt)}/${String(MAX_RETRY_ATTEMPTS)} failed: ${msg}`,
      );

      if (attempt < MAX_RETRY_ATTEMPTS) {
        console.log("[scheduler] Retrying in 15 minutes...");
        await new Promise((resolve) => setTimeout(resolve, RETRY_INTERVAL_MS));
      }
    }
  }
  console.error("[scheduler] All retry attempts exhausted");
};

/**
 * Schedule daily price fetch at 12:00 UTC (14:00 EET winter / 15:00 EET summer).
 * Nord Pool publishes next-day prices at ~12:42 CET (13:42 EET).
 * Running at 14:00 EET gives ~15 min buffer.
 */
export const startScheduler = (db: Database.Database): ScheduledTask => {
  console.log("[scheduler] Daily price fetch scheduled for 12:00 UTC");

  return cron.schedule("0 12 * * *", () => {
    void retryWithBackoff(db);
  });
};

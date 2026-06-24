import cron from "node-cron";
import type { Pool } from "pg";
import { runFetchJob } from "./fetch-job.js";
import { runForecastFetchJob } from "./forecast-job.js";
import { runWeatherFetchJob } from "./weather-job.js";
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

/**
 * Run the Fingrid grid-data fetch for the FI forecast.
 *
 * Wrapped in its own try/catch and isolated from `safeFetch` so a Fingrid
 * failure can NEVER affect the authoritative Nord Pool price cron or path
 * (STACK.md §9 — the only other allowed background task). The Fingrid boundary
 * already degrades rather than throwing; this is a second belt-and-braces
 * guard. No-op (a quiet log) when no API key is configured.
 */
const safeForecastFetch = async (
  pool: Pool,
  apiKey: string | undefined,
  label: string,
): Promise<void> => {
  if (!apiKey) {
    return;
  }
  try {
    const result = await runForecastFetchJob(pool, apiKey);
    if (result.ok) {
      const vintageNote =
        result.vintageDegradedReason === undefined
          ? `; vintages stored ${String(result.vintageStored)}, pruned ${String(result.vintagePruned)}`
          : `; vintage archival degraded — ${result.vintageDegradedReason}`;
      console.log(
        `[scheduler] ${label}: stored ${String(result.stored)} Fingrid rows, pruned ${String(result.pruned)}${vintageNote}`,
      );
    } else {
      console.warn(`[scheduler] ${label}: Fingrid degraded — ${result.reason}`);
    }
  } catch (error) {
    const msg = error instanceof Error ? error.message : "unknown error";
    console.error(`[scheduler] ${label} failed: ${msg}`);
  }
};

/**
 * Run an immediate Fingrid fetch on startup so the forecast has data after a
 * deploy/restart without waiting for the next hourly tick.
 */
export const runStartupForecastFetch = (
  pool: Pool,
  apiKey: string | undefined,
): void => {
  void safeForecastFetch(pool, apiKey, "Startup forecast fetch");
};

/**
 * Run the OpenWeatherMap weather data-collection fetch for the FI forecast
 * (issue #73, Phase 1).
 *
 * Wrapped in its own try/catch and isolated from `safeFetch`/`safeForecastFetch`
 * so a weather failure can NEVER affect the authoritative Nord Pool price cron
 * or path (STACK.md §9). The OWM boundary already degrades rather than throwing
 * and the job reports partial/total failure per point; this is a second
 * belt-and-braces guard. No-op when no API key is configured — and because the
 * cron is only scheduled when a key is present (and Playwright's webServer.env
 * whitelist does not pass the key), no live OWM call is ever made in tests/E2E,
 * which keeps the One Call 3.0 billing surface at zero outside production.
 */
const safeWeatherFetch = async (
  pool: Pool,
  apiKey: string | undefined,
  label: string,
): Promise<void> => {
  if (!apiKey) {
    return;
  }
  try {
    const result = await runWeatherFetchJob(pool, apiKey);
    if (result.status === "ok") {
      console.log(
        `[scheduler] ${label}: stored ${String(result.stored)} weather rows, pruned ${String(result.pruned)}`,
      );
    } else if (result.status === "partial") {
      const failed = result.failures.map((f) => f.pointId).join(", ");
      console.warn(
        `[scheduler] ${label}: stored ${String(result.stored)} weather rows, pruned ${String(result.pruned)}; degraded points: ${failed}`,
      );
    } else {
      const failed = result.failures.map((f) => f.pointId).join(", ");
      console.warn(
        `[scheduler] ${label}: weather degraded — all points failed: ${failed}`,
      );
    }
  } catch (error) {
    const msg = error instanceof Error ? error.message : "unknown error";
    console.error(`[scheduler] ${label} failed: ${msg}`);
  }
};

/**
 * Run an immediate weather fetch on startup so the collection table gains data
 * after a deploy/restart without waiting for the next hourly tick. No-op
 * without a key.
 */
export const runStartupWeatherFetch = (
  pool: Pool,
  apiKey: string | undefined,
): void => {
  void safeWeatherFetch(pool, apiKey, "Startup weather fetch");
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
export const startScheduler = (
  pool: Pool,
  fingridApiKey?: string,
  weatherApiKey?: string,
): SchedulerHandle => {
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

  // FI forecast: hourly Fingrid grid-data fetch. Isolated from the price crons
  // above — a Fingrid failure can never affect the authoritative price path
  // (STACK.md §9). Only scheduled when a key is configured.
  const forecast = fingridApiKey
    ? cron.schedule("0 * * * *", () => {
        void safeForecastFetch(pool, fingridApiKey, "Forecast fetch");
      })
    : null;

  if (forecast) {
    console.log("[scheduler] Forecast (Fingrid) fetch scheduled: hourly");
  } else {
    console.log(
      "[scheduler] Forecast (Fingrid) fetch disabled: no FINGRID_API_KEY",
    );
  }

  // FI forecast: hourly OpenWeatherMap weather collection (issue #73 Phase 1).
  // Isolated from every cron above — a weather failure can never affect the
  // authoritative price path (STACK.md §9). Only scheduled when a key is
  // configured, so no live (billable) One Call 3.0 request is made otherwise.
  const weather = weatherApiKey
    ? cron.schedule("0 * * * *", () => {
        void safeWeatherFetch(pool, weatherApiKey, "Weather fetch");
      })
    : null;

  if (weather) {
    console.log("[scheduler] Weather (OpenWeatherMap) fetch scheduled: hourly");
  } else {
    console.log(
      "[scheduler] Weather (OpenWeatherMap) fetch disabled: no OPENWEATHERMAP_API_KEY",
    );
  }

  return {
    stop: () => {
      void standard.stop();
      void burst.stop();
      if (forecast) {
        void forecast.stop();
      }
      if (weather) {
        void weather.stop();
      }
    },
  };
};

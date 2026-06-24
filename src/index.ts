import { serve } from "@hono/node-server";
import { createApp } from "./app.js";
import { createAuth } from "./auth.js";
import { initDatabase, closeDatabase } from "./db.js";
import { loadEnv } from "./env.js";
import {
  startScheduler,
  runStartupFetch,
  runStartupForecastFetch,
  runStartupWeatherFetch,
} from "./scheduler.js";

const main = async (): Promise<void> => {
  // Validate the environment up front so the process fails fast with a clear
  // error before any service is constructed or the HTTP listener opens.
  const env = loadEnv();

  // The FI forecast is optional. When it is disabled in production make that
  // loud in the Railway logs, rather than letting the endpoint silently answer
  // available:false with no explanation.
  if (env.NODE_ENV === "production" && !env.FINGRID_API_KEY) {
    console.warn(
      "Forecast disabled: FINGRID_API_KEY not set — /api/v1/price/forecast will return available:false",
    );
  }

  // Weather collection (issue #73 Phase 1) is optional too. When disabled in
  // production make it loud in the Railway logs rather than silently dropping
  // the forward-only weather history that future forecast phases depend on.
  if (env.NODE_ENV === "production" && !env.OPENWEATHERMAP_API_KEY) {
    console.warn(
      "Weather collection disabled: OPENWEATHERMAP_API_KEY not set — weather_forecasts will not accumulate",
    );
  }

  const pool = await initDatabase();
  const auth = createAuth(pool);
  const app = createApp(pool, auth);
  const port = env.PORT;

  // Start price fetch scheduler (2h baseline + 10min burst during publication window)
  // plus the hourly Fingrid forecast fetch when a key is configured.
  const scheduler = startScheduler(
    pool,
    env.FINGRID_API_KEY,
    env.OPENWEATHERMAP_API_KEY,
  );
  runStartupFetch(pool);
  runStartupForecastFetch(pool, env.FINGRID_API_KEY);
  runStartupWeatherFetch(pool, env.OPENWEATHERMAP_API_KEY);

  // Graceful shutdown: stop scheduler, close DB, then exit
  const shutdown = async (): Promise<void> => {
    console.log("Shutting down gracefully...");
    scheduler.stop();
    await closeDatabase(pool);
    process.exit(0);
  };

  process.on("SIGTERM", () => void shutdown());
  process.on("SIGINT", () => void shutdown());

  serve({ fetch: app.fetch, port }, (info) => {
    console.log(`Server running on http://localhost:${String(info.port)}`);
  });
};

void main();

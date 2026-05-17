import { serve } from "@hono/node-server";
import { createApp } from "./app.js";
import { createAuth } from "./auth.js";
import { initDatabase, closeDatabase } from "./db.js";
import { loadEnv } from "./env.js";
import { startScheduler, runStartupFetch } from "./scheduler.js";

const main = async (): Promise<void> => {
  // Validate the environment up front so the process fails fast with a clear
  // error before any service is constructed or the HTTP listener opens.
  const env = loadEnv();

  const pool = await initDatabase();
  const auth = createAuth(pool);
  const app = createApp(pool, auth);
  const port = env.PORT;

  // Start price fetch scheduler (2h baseline + 10min burst during publication window)
  const scheduler = startScheduler(pool);
  runStartupFetch(pool);

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

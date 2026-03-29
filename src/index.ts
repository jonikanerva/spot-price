import { serve } from "@hono/node-server";
import { createApp } from "./app.js";
import { createAuth } from "./auth.js";
import { initDatabase, closeDatabase } from "./db.js";
import { startScheduler, runStartupFetch } from "./scheduler.js";

const DEFAULT_PORT = 3000;

const getPort = (): number => {
  const envPort = process.env["PORT"];
  if (envPort) {
    const parsed = parseInt(envPort, 10);
    if (!isNaN(parsed) && parsed > 0) {
      return parsed;
    }
  }
  return DEFAULT_PORT;
};

const main = async (): Promise<void> => {
  const pool = await initDatabase();
  const auth = createAuth(pool);
  const app = createApp(pool, auth);
  const port = getPort();

  // Start price fetch scheduler (every 2 hours) + immediate startup fetch
  const schedulerTask = startScheduler(pool);
  runStartupFetch(pool);

  // Graceful shutdown: stop scheduler, close DB, then exit
  const shutdown = async (): Promise<void> => {
    console.log("Shutting down gracefully...");
    void schedulerTask.stop();
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

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

const main = (): void => {
  const db = initDatabase();
  const auth = createAuth(db);
  const app = createApp(db, auth);
  const port = getPort();

  // Start price fetch scheduler (every 2 hours) + immediate startup fetch
  const schedulerTask = startScheduler(db);
  runStartupFetch(db);

  // Graceful shutdown: stop scheduler, close DB
  const shutdown = (): void => {
    console.log("Shutting down gracefully...");
    void schedulerTask.stop();
    closeDatabase(db);
    process.exit(0);
  };

  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);

  serve({ fetch: app.fetch, port }, (info) => {
    console.log(`Server running on http://localhost:${String(info.port)}`);
  });
};

main();

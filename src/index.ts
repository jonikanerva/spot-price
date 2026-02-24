import { serve } from "@hono/node-server";
import { createApp } from "./app.js";
import { initDatabase, closeDatabase } from "./db.js";

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
  const app = createApp(db);
  const port = getPort();

  // Graceful shutdown: close DB on SIGTERM/SIGINT
  const shutdown = (): void => {
    console.log("Shutting down gracefully...");
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

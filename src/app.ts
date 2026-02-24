import { Hono } from "hono";
import { logger } from "hono/logger";
import type Database from "better-sqlite3";

export interface AppEnv {
  Variables: {
    db: Database.Database;
  };
}

export const createApp = (db: Database.Database): Hono<AppEnv> => {
  const app = new Hono<AppEnv>();

  // Middleware: request logging
  app.use(logger());

  // Middleware: inject database into context
  app.use(async (c, next) => {
    c.set("db", db);
    await next();
  });

  // Health check — verifies DB is accessible
  app.get("/health", (c) => {
    try {
      const dbInstance = c.get("db");
      const result = dbInstance.prepare("SELECT 1 as ok").get() as
        | { ok: number }
        | undefined;

      if (result?.ok === 1) {
        return c.json({ status: "ok", db: "connected" });
      }
      return c.json({ status: "error", db: "query failed" }, 503);
    } catch {
      return c.json({ status: "error", db: "unavailable" }, 503);
    }
  });

  return app;
};

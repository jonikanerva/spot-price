import { createApp } from "./app.js";
import { createAuth } from "./auth.js";
import { initTestDatabase } from "./db.js";
import type Database from "better-sqlite3";

/** Create a test app with an in-memory database and auth instance. */
export const createTestApp = (
  db: Database.Database,
): ReturnType<typeof createApp> => {
  const auth = createAuth(db);
  return createApp(db, auth);
};

/** Create a test database and app together. */
export const initTestApp = (): {
  db: Database.Database;
  app: ReturnType<typeof createApp>;
} => {
  const db = initTestDatabase();
  const app = createTestApp(db);
  return { db, app };
};

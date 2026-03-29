import { createApp } from "./app.js";
import { createAuth } from "./auth.js";
import type Database from "better-sqlite3";

/** Create a test app with an in-memory database and auth instance. */
export const createTestApp = (
  db: Database.Database,
): ReturnType<typeof createApp> => {
  const auth = createAuth(db);
  return createApp(db, auth);
};

import { createApp } from "./app.js";
import { createAuth } from "./auth.js";
import type { Pool } from "pg";

/** Create a test app with a PostgreSQL pool and auth instance. */
export const createTestApp = (
  pool: Pool,
): ReturnType<typeof createApp> => {
  const auth = createAuth(pool);
  return createApp(pool, auth);
};

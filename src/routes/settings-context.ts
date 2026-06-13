import type { Context } from "hono";
import type { AppEnv } from "../app.js";
import { getUserSettings } from "../user-settings.js";
import type { UserSettings } from "../types.js";

/**
 * Resolve the authenticated caller's contract settings from the request context.
 * Returns null when the user has no settings row; callers shape their own 404.
 * Safe to call only on routes mounted behind `apiKeyAuth` (app.ts:95), which
 * guarantees a truthy `userId`; on those paths `db` and `userId` are present by
 * AppEnv typing, so no runtime type guard is needed.
 */
export const getUserSettingsFromContext = (
  c: Context<AppEnv>,
): Promise<UserSettings | null> =>
  getUserSettings(c.get("db"), c.get("userId"));

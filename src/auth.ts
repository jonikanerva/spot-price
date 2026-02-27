import type Database from "better-sqlite3";
import { betterAuth } from "better-auth";
import type { BetterAuthOptions } from "better-auth";

/** Create a Better Auth instance backed by the given SQLite database.
 *  Call once at startup with the same database used by the rest of the app. */
export const createAuth = (
  db: Database.Database,
): ReturnType<typeof betterAuth> => {
  const options: BetterAuthOptions = {
    baseURL: process.env["BETTER_AUTH_URL"] ?? "http://localhost:3000",
    secret:
      process.env["BETTER_AUTH_SECRET"] ??
      "dev-only-better-auth-secret-must-be-32+",
    database: db,
    emailAndPassword: {
      enabled: true,
      autoSignIn: true,
    },
  };
  return betterAuth(options);
};

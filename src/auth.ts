import type Database from "better-sqlite3";
import { betterAuth } from "better-auth";
import type { BetterAuthOptions } from "better-auth";

const DEFAULT_LOCAL_AUTH_URL = "http://localhost:3000";

const requireProductionEnv = (name: string): string => {
  const value = process.env[name];
  if (!value || value.trim().length === 0) {
    throw new Error(`${name} is required in production`);
  }
  return value;
};

/** Create a Better Auth instance backed by the given SQLite database.
 *  Call once at startup with the same database used by the rest of the app. */
export const createAuth = (
  db: Database.Database,
): ReturnType<typeof betterAuth> => {
  const isProduction = process.env["NODE_ENV"] === "production";
  const baseURL = isProduction
    ? requireProductionEnv("BETTER_AUTH_URL")
    : (process.env["BETTER_AUTH_URL"] ?? DEFAULT_LOCAL_AUTH_URL);
  const secret = isProduction
    ? requireProductionEnv("BETTER_AUTH_SECRET")
    : (process.env["BETTER_AUTH_SECRET"] ??
      "dev-only-better-auth-secret-must-be-32+");

  const options: BetterAuthOptions = {
    baseURL,
    secret,
    database: db,
    emailAndPassword: {
      enabled: true,
      autoSignIn: true,
    },
  };
  return betterAuth(options);
};

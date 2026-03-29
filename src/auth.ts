import type { Pool } from "pg";
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

/** Create a Better Auth instance backed by the given PostgreSQL pool.
 *  Call once at startup with the same pool used by the rest of the app. */
export const createAuth = (
  pool: Pool,
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
    database: pool,
    emailAndPassword: {
      enabled: true,
      autoSignIn: true,
    },
  };
  return betterAuth(options);
};

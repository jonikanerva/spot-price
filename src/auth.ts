import type { Pool } from "pg";
import { betterAuth } from "better-auth";
import type { BetterAuthOptions } from "better-auth";
import { loadEnv } from "./env.js";

const DEFAULT_LOCAL_AUTH_URL = "http://localhost:3000";
const DEV_FALLBACK_SECRET = "dev-only-better-auth-secret-must-be-32+";

/** Create a Better Auth instance backed by the given PostgreSQL pool.
 *  Call once at startup with the same pool used by the rest of the app. */
export const createAuth = (pool: Pool): ReturnType<typeof betterAuth> => {
  const env = loadEnv();
  const isProduction = env.NODE_ENV === "production";

  // loadEnv() has already enforced the production-required env checks; the
  // narrowings below cover the development branch where the values are
  // genuinely optional.
  const baseURL = isProduction
    ? (env.BETTER_AUTH_URL ?? "") // unreachable: loadEnv throws in prod when missing
    : (env.BETTER_AUTH_URL ?? DEFAULT_LOCAL_AUTH_URL);
  const secret = isProduction
    ? (env.BETTER_AUTH_SECRET ?? "") // unreachable: loadEnv throws in prod when missing
    : (env.BETTER_AUTH_SECRET ?? DEV_FALLBACK_SECRET);

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

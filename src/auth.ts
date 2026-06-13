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
    // reason: VISION.md → Persistence and Privacy Posture ("Not stored: ...
    // IPs beyond what in-memory rate limiting needs"). better-auth 1.6.11 has
    // no native flag to disable userAgent persistence, and we deliberately do
    // NOT set advanced.ipAddress.disableIpTracking so Better Auth's own
    // internal rate limiter can still compute a transient IP for keying. This
    // before-hook is therefore the durable mechanism that guarantees the
    // STORED session row never retains the IP or user agent: the runtime
    // merges `{ ...session, ...result.data }`, so returning the two nulled
    // fields (the `{ data }` wrapper is mandatory — a bare object is a no-op,
    // and any falsy return aborts session creation) scrubs them before insert.
    databaseHooks: {
      session: {
        create: {
          before: () =>
            Promise.resolve({ data: { ipAddress: null, userAgent: null } }),
        },
      },
    },
  };
  return betterAuth(options);
};

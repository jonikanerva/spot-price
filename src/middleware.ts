import type { Context, Next } from "hono";
import { rateLimiter } from "hono-rate-limiter";
import { resolveApiKey } from "./api-keys.js";
import type { AppEnv } from "./app.js";

/** Extract client IP from request headers (Railway sets X-Real-IP) */
export const getClientIp = (c: Context): string =>
  c.req.header("x-real-ip") ??
  c.req.header("x-forwarded-for")?.split(",")[0]?.trim() ??
  "unknown";

/** Global IP rate limit: 120 req/min per IP, skip /health */
export const globalRateLimit = rateLimiter<AppEnv>({
  windowMs: 60_000,
  limit: 120,
  keyGenerator: (c) => getClientIp(c),
  skip: (c) => c.req.path === "/health",
  standardHeaders: "draft-6",
  message: { error: "Too many requests. Try again later." },
});

/** Login/signup rate limit: 10 req/15min per IP */
export const loginRateLimit = rateLimiter<AppEnv>({
  windowMs: 15 * 60_000,
  limit: 10,
  keyGenerator: (c) => `login:${getClientIp(c)}`,
  standardHeaders: "draft-6",
  message: { error: "Too many login attempts. Try again in 15 minutes." },
});

/** API key rate limit: 60 req/min per key (must run after apiKeyAuth) */
export const apiKeyRateLimit = rateLimiter<AppEnv>({
  windowMs: 60_000,
  limit: 60,
  keyGenerator: (c) => `apikey:${c.get("userId")}`,
  standardHeaders: "draft-6",
  message: { error: "Rate limit exceeded. Try again later." },
});

/** Extract Bearer token from Authorization header */
const extractBearerToken = (header: string | undefined): string | null => {
  if (!header) {
    return null;
  }
  const match = /^Bearer\s+(\S+)$/i.exec(header);
  return match?.[1] ?? null;
};

/** API key authentication middleware */
export const apiKeyAuth = async (
  c: Context<AppEnv>,
  next: Next,
): Promise<Response | undefined> => {
  const token = extractBearerToken(c.req.header("Authorization"));

  if (!token) {
    return c.json(
      {
        error: "Missing or invalid Authorization header. Use: Bearer <api-key>",
      },
      401,
    );
  }

  const db = c.get("db");
  const userId = resolveApiKey(db, token);

  if (!userId) {
    return c.json({ error: "Invalid API key" }, 401);
  }

  c.set("userId", userId);
  await next();
  return undefined;
};

/** Maximum number of users allowed in the system */
export const MAX_USERS = 100;

/** Check if user registration is open (under the user cap) */
export const isRegistrationOpen = (db: {
  prepare: (sql: string) => { get: () => { count: number } | undefined };
}): boolean => {
  const row = db.prepare('SELECT COUNT(*) as count FROM "user"').get() as
    | { count: number }
    | undefined;
  return (row?.count ?? 0) < MAX_USERS;
};

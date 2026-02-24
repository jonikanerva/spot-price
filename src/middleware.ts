import type { Context, Next } from "hono";
import { resolveApiKey } from "./api-keys.js";
import type { AppEnv } from "./app.js";

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

/** Simple in-memory rate limiter (per API key) */
interface RateLimitEntry {
  count: number;
  resetAt: number;
}

const rateLimitStore = new Map<string, RateLimitEntry>();
const WINDOW_MS = 60_000; // 1 minute
const MAX_REQUESTS = 60; // 60 req/min

/** Clean expired entries periodically */
const cleanExpired = (): void => {
  const now = Date.now();
  for (const [key, entry] of rateLimitStore) {
    if (entry.resetAt <= now) {
      rateLimitStore.delete(key);
    }
  }
};

// Clean every 5 minutes
setInterval(cleanExpired, 5 * 60_000);

/** Rate limiting middleware (must run after apiKeyAuth) */
export const rateLimit = async (
  c: Context<AppEnv>,
  next: Next,
): Promise<Response | undefined> => {
  const userId = c.get("userId");
  const now = Date.now();
  const existing = rateLimitStore.get(userId);

  if (existing && existing.resetAt > now) {
    if (existing.count >= MAX_REQUESTS) {
      const retryAfter = Math.ceil((existing.resetAt - now) / 1000);
      c.header("Retry-After", String(retryAfter));
      return c.json({ error: "Rate limit exceeded. Try again later." }, 429);
    }
    existing.count++;
  } else {
    rateLimitStore.set(userId, {
      count: 1,
      resetAt: now + WINDOW_MS,
    });
  }

  await next();
  return undefined;
};

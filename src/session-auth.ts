import type { Context, Next } from "hono";
import type { AppEnv } from "./app.js";

export interface AuthSessionUser {
  readonly id: string;
  readonly email: string;
  readonly name: string;
}

interface BetterAuthSessionPayload {
  readonly user?: {
    readonly id: string;
    readonly email: string;
    readonly name: string;
  };
}

/** Create session auth middleware that uses the given Better Auth instance. */
export const createSessionAuth = (authInstance: {
  api: { getSession: (opts: { headers: Headers }) => Promise<unknown> };
}) => {
  return async (
    c: Context<AppEnv>,
    next: Next,
  ): Promise<Response | undefined> => {
    const session = (await authInstance.api.getSession({
      headers: c.req.raw.headers,
    })) as BetterAuthSessionPayload | null;

    if (!session?.user) {
      return c.json({ error: "Unauthorized" }, 401);
    }

    c.set("sessionUser", session.user);
    await next();
    return undefined;
  };
};

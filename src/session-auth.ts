import type { Context, Next } from "hono";
import type { AppEnv } from "./app.js";
import { auth } from "./auth.js";

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

export const sessionAuth = async (
  c: Context<AppEnv>,
  next: Next,
): Promise<Response | undefined> => {
  const session = (await auth.api.getSession({
    headers: c.req.raw.headers,
  })) as BetterAuthSessionPayload | null;

  if (!session?.user) {
    return c.json({ error: "Unauthorized" }, 401);
  }

  c.set("sessionUser", session.user);
  await next();
  return undefined;
};

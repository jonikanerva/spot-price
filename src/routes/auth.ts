import type { OpenAPIHono } from "@hono/zod-openapi";
import { getCurrentApiKey, regenerateApiKey } from "../api-keys.js";
import { loginRateLimit } from "../middleware.js";
import { ensureUserSettings } from "../user-settings.js";
import {
  assignUsername,
  getUserIdByUsername,
  getUsernameByUserId,
  normalizeUsername,
  toInternalEmail,
  validateUsername,
} from "../usernames.js";
import { isRegistrationOpen } from "../middleware.js";
import { createSessionAuth } from "../session-auth.js";
import type { AppEnv, AuthInstance } from "../app.js";

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

export const registerAuthRoutes = (
  app: OpenAPIHono<AppEnv>,
  auth: AuthInstance,
): void => {
  const sessionAuth = createSessionAuth(auth);

  // --- Login / Signup -------------------------------------------------------

  app.post("/api/session/login-or-signup", loginRateLimit, async (c) => {
    const payload = await (async (): Promise<{
      username?: string;
      password?: string;
    } | null> => {
      try {
        return await c.req.json<{
          username?: string;
          password?: string;
        }>();
      } catch {
        return null;
      }
    })();

    if (!payload || typeof payload !== "object") {
      return c.json({ error: "Invalid JSON payload" }, 400);
    }

    const username = normalizeUsername(payload.username ?? "");
    const password = payload.password;

    if (!validateUsername(username) || !password) {
      return c.json(
        {
          error:
            "username must match [a-z0-9_-] and be 3-32 chars; password is required",
        },
        400,
      );
    }

    const existingUserId = await getUserIdByUsername(c.get("db"), username);
    const email = toInternalEmail(username);

    if (existingUserId) {
      const signInResponse = await auth.api.signInEmail({
        headers: c.req.raw.headers,
        body: { email, password, rememberMe: true },
        asResponse: true,
      });
      return signInResponse;
    }

    if (!(await isRegistrationOpen(c.get("db")))) {
      return c.json({ error: "Registration is currently closed." }, 403);
    }

    const signUpResponse = await auth.api.signUpEmail({
      headers: c.req.raw.headers,
      body: { email, password, name: username },
      asResponse: true,
    });

    if (signUpResponse.ok) {
      const { rows } = await c.get("db").query<{
        id: string;
      }>('SELECT id FROM "user" WHERE email = $1', [email]);
      const newUserId = rows[0]?.id;
      if (newUserId) {
        await assignUsername(c.get("db"), newUserId, username);
        await ensureUserSettings(c.get("db"), newUserId);
      }
    }

    return signUpResponse;
  });

  // --- Sign out --------------------------------------------------------------

  app.post("/api/session/sign-out", async (c) => {
    const response = await auth.api.signOut({
      headers: c.req.raw.headers,
      asResponse: true,
    });
    return response;
  });

  // --- Session info ----------------------------------------------------------

  app.get("/api/session", async (c) => {
    const session = await auth.api.getSession({
      headers: c.req.raw.headers,
    });
    const userId =
      (session as { user?: { id?: string } } | null)?.user?.id ?? null;
    const username = userId
      ? await getUsernameByUserId(c.get("db"), userId)
      : null;
    return c.json({ session, username });
  });

  // --- API key management (session-protected) --------------------------------

  app.use("/api/keys", sessionAuth);
  app.use("/api/keys/*", sessionAuth);

  app.get("/api/keys", async (c) => {
    const userId = c.get("sessionUser").id;
    await ensureUserSettings(c.get("db"), userId);
    const existing = await getCurrentApiKey(c.get("db"), userId);
    if (existing) {
      return c.json({ apiKey: existing.key, createdAt: existing.createdAt });
    }
    // No key exists at all — create the first one
    const created = await regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt }, 201);
  });

  app.post("/api/keys/regenerate", async (c) => {
    const userId = c.get("sessionUser").id;
    await ensureUserSettings(c.get("db"), userId);
    const created = await regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt });
  });
};

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

    const existingUserId = getUserIdByUsername(c.get("db"), username);
    const email = toInternalEmail(username);

    if (existingUserId) {
      const signInResponse = await auth.api.signInEmail({
        headers: c.req.raw.headers,
        body: { email, password, rememberMe: true },
        asResponse: true,
      });
      return signInResponse;
    }

    if (!isRegistrationOpen(c.get("db"))) {
      return c.json({ error: "Registration is currently closed." }, 403);
    }

    const signUpResponse = await auth.api.signUpEmail({
      headers: c.req.raw.headers,
      body: { email, password, name: username },
      asResponse: true,
    });

    if (signUpResponse.ok) {
      const newUserRow = c
        .get("db")
        .prepare('SELECT id FROM "user" WHERE email = ?')
        .get(email) as { id: string } | undefined;
      if (newUserRow?.id) {
        assignUsername(c.get("db"), newUserRow.id, username);
        ensureUserSettings(c.get("db"), newUserRow.id);
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
    const username = userId ? getUsernameByUserId(c.get("db"), userId) : null;
    return c.json({ session, username });
  });

  // --- API key management (session-protected) --------------------------------

  app.use("/api/keys", sessionAuth);
  app.use("/api/keys/*", sessionAuth);

  app.get("/api/keys", (c) => {
    const userId = c.get("sessionUser").id;
    ensureUserSettings(c.get("db"), userId);
    const existing = getCurrentApiKey(c.get("db"), userId);
    if (existing) {
      return c.json({ apiKey: existing.key, createdAt: existing.createdAt });
    }
    // No key exists at all — create the first one
    const created = regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt }, 201);
  });

  app.post("/api/keys/regenerate", (c) => {
    const userId = c.get("sessionUser").id;
    ensureUserSettings(c.get("db"), userId);
    const created = regenerateApiKey(c.get("db"), userId);
    return c.json({ apiKey: created.key, createdAt: created.createdAt });
  });
};

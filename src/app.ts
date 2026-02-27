import { OpenAPIHono } from "@hono/zod-openapi";
import { Scalar } from "@scalar/hono-api-reference";
import { logger } from "hono/logger";
import type Database from "better-sqlite3";
import { apiKeyAuth, apiKeyRateLimit, globalRateLimit } from "./middleware.js";
import { renderHomePage } from "./ui.js";
import type { AuthSessionUser } from "./session-auth.js";
import { createSessionAuth } from "./session-auth.js";
import { registerAuthRoutes } from "./routes/auth.js";
import { registerPriceRoutes } from "./routes/price.js";
import { registerUserRoutes } from "./routes/user.js";

export interface AppEnv {
  Variables: {
    db: Database.Database;
    userId: string;
    sessionUser: AuthSessionUser;
  };
}

export interface AuthInstance {
  readonly api: {
    readonly signInEmail: (opts: {
      headers: Headers;
      body: { email: string; password: string; rememberMe: boolean };
      asResponse: true;
    }) => Promise<Response>;
    readonly signUpEmail: (opts: {
      headers: Headers;
      body: { email: string; password: string; name: string };
      asResponse: true;
    }) => Promise<Response>;
    readonly signOut: (opts: {
      headers: Headers;
      asResponse: true;
    }) => Promise<Response>;
    readonly getSession: (opts: { headers: Headers }) => Promise<unknown>;
  };
}

export const createApp = (
  db: Database.Database,
  auth: AuthInstance,
): OpenAPIHono<AppEnv> => {
  const sessionAuth = createSessionAuth(auth);
  const app = new OpenAPIHono<AppEnv>({
    defaultHook: (result, c) => {
      if (!result.success) {
        const messages = result.error.issues
          .map((i) =>
            i.path.length > 0 ? `${i.path.join(".")}: ${i.message}` : i.message,
          )
          .join("; ");
        return c.json({ error: messages }, 400);
      }
    },
  });

  // --- Middleware ----------------------------------------------------------

  app.use(logger());

  app.use(async (c, next) => {
    c.set("db", db);
    await next();
  });

  app.use(globalRateLimit);

  // --- Non-OpenAPI routes (health, HTML) -----------------------------------

  app.get("/health", (c) => {
    try {
      const dbInstance = c.get("db");
      const result = dbInstance.prepare("SELECT 1 as ok").get() as
        | { ok: number }
        | undefined;

      if (result?.ok === 1) {
        return c.json({ status: "ok", db: "connected" });
      }
      return c.json({ status: "error", db: "query failed" }, 503);
    } catch {
      return c.json({ status: "error", db: "unavailable" }, 503);
    }
  });

  app.get("/", (c) => c.html(renderHomePage()));

  // --- Auth & key management routes (non-OpenAPI) --------------------------

  registerAuthRoutes(app, auth);

  // --- Auth middleware for API routes --------------------------------------

  app.use("/api/v1/price/*", apiKeyAuth, apiKeyRateLimit);
  app.use("/api/v1/me/*", sessionAuth);

  // --- OpenAPI routes ------------------------------------------------------

  registerPriceRoutes(app);
  registerUserRoutes(app);

  // --- OpenAPI spec + interactive docs ------------------------------------

  app.doc31("/api/v1/openapi.json", {
    openapi: "3.1.0",
    info: {
      title: "Spot Price API",
      version: "1.0.0",
      description:
        "Finnish and Nordic spot electricity price API with total price calculation, cheapest window finder, and per-user contract settings.",
    },
    servers: [{ url: "https://spot.calmdonut.com" }],
    security: [{ BearerAuth: [] }],
    tags: [
      {
        name: "Price",
        description: "Electricity price endpoints (API key required)",
      },
      {
        name: "User",
        description: "User settings and chart data (session required)",
      },
      { name: "Public", description: "Unauthenticated endpoints" },
    ],
  });

  app.openAPIRegistry.registerComponent("securitySchemes", "BearerAuth", {
    type: "http",
    scheme: "bearer",
    description:
      "API key obtained from the web dashboard. Use as: Authorization: Bearer <api-key>",
  });

  app.get(
    "/api/docs",
    Scalar({
      url: "/api/v1/openapi.json",
      theme: "deepSpace",
    }),
  );

  return app;
};

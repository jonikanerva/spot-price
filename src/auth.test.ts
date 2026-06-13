import { afterEach, describe, expect, it } from "vitest";
import type { Pool } from "pg";
import { createAuth } from "./auth.js";
import { createTestApp } from "./test-utils.js";
import { closeDatabase, initTestDatabase } from "./db.js";

const ORIGINAL_ENV = {
  NODE_ENV: process.env["NODE_ENV"],
  BETTER_AUTH_URL: process.env["BETTER_AUTH_URL"],
  BETTER_AUTH_SECRET: process.env["BETTER_AUTH_SECRET"],
};

const restoreEnv = (): void => {
  if (ORIGINAL_ENV.NODE_ENV === undefined) {
    delete process.env.NODE_ENV;
  } else {
    process.env.NODE_ENV = ORIGINAL_ENV.NODE_ENV;
  }

  if (ORIGINAL_ENV.BETTER_AUTH_URL === undefined) {
    delete process.env.BETTER_AUTH_URL;
  } else {
    process.env.BETTER_AUTH_URL = ORIGINAL_ENV.BETTER_AUTH_URL;
  }

  if (ORIGINAL_ENV.BETTER_AUTH_SECRET === undefined) {
    delete process.env.BETTER_AUTH_SECRET;
  } else {
    process.env.BETTER_AUTH_SECRET = ORIGINAL_ENV.BETTER_AUTH_SECRET;
  }
};

describe("createAuth", () => {
  let pool: Pool;

  afterEach(async () => {
    restoreEnv();
    await closeDatabase(pool);
  });

  it("throws in production when BETTER_AUTH_SECRET is missing", async () => {
    pool = await initTestDatabase();
    process.env["NODE_ENV"] = "production";
    process.env["BETTER_AUTH_URL"] = "https://spot.example.com";
    delete process.env["BETTER_AUTH_SECRET"];

    expect(() => createAuth(pool)).toThrow(
      "BETTER_AUTH_SECRET is required in production",
    );
  });

  it("throws in production when BETTER_AUTH_URL is missing", async () => {
    pool = await initTestDatabase();
    process.env["NODE_ENV"] = "production";
    process.env["BETTER_AUTH_SECRET"] = "a-very-long-production-secret-value";
    delete process.env["BETTER_AUTH_URL"];

    expect(() => createAuth(pool)).toThrow(
      "BETTER_AUTH_URL is required in production",
    );
  });

  it("allows dev mode defaults", async () => {
    pool = await initTestDatabase();
    process.env["NODE_ENV"] = "development";
    delete process.env["BETTER_AUTH_URL"];
    delete process.env["BETTER_AUTH_SECRET"];

    expect(() => createAuth(pool)).not.toThrow();
  });

  // VISION.md → Persistence and Privacy Posture ("Not stored: ... IPs ..."):
  // a real signup through the configured Better Auth instance must persist a
  // session row whose ipAddress AND userAgent are NULL. userAgent is the
  // regression-prone field (better-auth 1.6.11 has no native disable flag), so
  // this drives the full auth flow rather than calling the hook in isolation.
  it("never persists session ipAddress or userAgent", async () => {
    pool = await initTestDatabase();
    process.env["NODE_ENV"] = "development";

    const app = createTestApp(pool);

    const response = await app.request("/api/session/login-or-signup", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        // Provide a user agent so we prove the hook nulls a real value rather
        // than relying on the client simply not sending one.
        "User-Agent": "spot-price-test-agent/1.0",
        "X-Forwarded-For": "203.0.113.7",
      },
      body: JSON.stringify({
        username: `user_${String(Date.now())}`,
        password: "password1234",
      }),
    });
    expect(response.status).toBe(200);

    const { rows } = await pool.query<{
      ipAddress: string | null;
      userAgent: string | null;
    }>('SELECT "ipAddress", "userAgent" FROM "session"');

    expect(rows.length).toBeGreaterThan(0);
    for (const row of rows) {
      // null OR empty string both satisfy "not persisted"; the hook returns null.
      expect(row.ipAddress ?? "").toBe("");
      expect(row.userAgent ?? "").toBe("");
    }
  });
});

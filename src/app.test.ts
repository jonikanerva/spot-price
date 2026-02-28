import { describe, it, expect, afterEach } from "vitest";
import Database from "better-sqlite3";
import { initTestDatabase, closeDatabase } from "./db.js";
import { createTestApp } from "./test-utils.js";
import { isRegistrationOpen, MAX_USERS, getClientIp } from "./middleware.js";
import { Hono } from "hono";

describe("health endpoint", () => {
  let db: Database.Database;

  afterEach(() => {
    try {
      closeDatabase(db);
    } catch {
      // DB may already be closed by test
    }
  });

  it("returns 200 with ok status when DB is healthy", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);

    const res = await app.request("/health");

    expect(res.status).toBe(200);
    const body = (await res.json()) as { status: string; db: string };
    expect(body.status).toBe("ok");
    expect(body.db).toBe("connected");
  });

  it("returns 503 when DB is closed", async () => {
    db = initTestDatabase();
    const app = createTestApp(db);
    db.close();

    const res = await app.request("/health");

    expect(res.status).toBe(503);
    const body = (await res.json()) as { status: string; db: string };
    expect(body.status).toBe("error");
  });
});

describe("isRegistrationOpen", () => {
  interface RegistrationCountDb {
    prepare: (sql: string) => { get: () => { count: number } | undefined };
  }

  const mockDbWithCount = (count: number): RegistrationCountDb => ({
    prepare: () => ({ get: () => ({ count }) }),
  });

  it("returns true when user count is below MAX_USERS", () => {
    expect(isRegistrationOpen(mockDbWithCount(0))).toBe(true);
  });

  it("returns false when user count reaches MAX_USERS", () => {
    expect(isRegistrationOpen(mockDbWithCount(MAX_USERS))).toBe(false);
  });

  it("returns true when just below the cap", () => {
    expect(isRegistrationOpen(mockDbWithCount(MAX_USERS - 1))).toBe(true);
  });
});

describe("getClientIp", () => {
  it("prefers x-real-ip header", async () => {
    const app = new Hono();
    let extractedIp = "";
    app.get("/test", (c) => {
      extractedIp = getClientIp(c);
      return c.text("ok");
    });

    await app.request("/test", {
      headers: {
        "x-real-ip": "1.2.3.4",
        "x-forwarded-for": "5.6.7.8, 9.10.11.12",
      },
    });

    expect(extractedIp).toBe("1.2.3.4");
  });

  it("falls back to x-forwarded-for first entry", async () => {
    const app = new Hono();
    let extractedIp = "";
    app.get("/test", (c) => {
      extractedIp = getClientIp(c);
      return c.text("ok");
    });

    await app.request("/test", {
      headers: {
        "x-forwarded-for": "5.6.7.8, 9.10.11.12",
      },
    });

    expect(extractedIp).toBe("5.6.7.8");
  });

  it("returns unknown when no IP headers present", async () => {
    const app = new Hono();
    let extractedIp = "";
    app.get("/test", (c) => {
      extractedIp = getClientIp(c);
      return c.text("ok");
    });

    await app.request("/test");

    expect(extractedIp).toBe("unknown");
  });
});

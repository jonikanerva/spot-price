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
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("returns true when user count is below MAX_USERS", () => {
    db = initTestDatabase();
    expect(isRegistrationOpen(db)).toBe(true);
  });

  it("returns false when user count reaches MAX_USERS", () => {
    db = initTestDatabase();

    // Insert MAX_USERS rows into the user table
    const now = new Date().toISOString();
    const insert = db.prepare(
      `INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    );
    for (let i = 0; i < MAX_USERS; i++) {
      insert.run(
        `user-${String(i)}`,
        `user${String(i)}`,
        `u${String(i)}@x.com`,
        now,
        now,
      );
    }

    expect(isRegistrationOpen(db)).toBe(false);
  });

  it("returns true when just below the cap", () => {
    db = initTestDatabase();

    const now = new Date().toISOString();
    const insert = db.prepare(
      `INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    );
    for (let i = 0; i < MAX_USERS - 1; i++) {
      insert.run(
        `user-${String(i)}`,
        `user${String(i)}`,
        `u${String(i)}@x.com`,
        now,
        now,
      );
    }

    expect(isRegistrationOpen(db)).toBe(true);
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

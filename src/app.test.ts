import { describe, it, expect, afterEach } from "vitest";
import Database from "better-sqlite3";
import { createApp } from "./app.js";

const createTestDb = (): Database.Database => {
  const db = new Database(":memory:");
  db.pragma("journal_mode = WAL");
  return db;
};

describe("health endpoint", () => {
  let db: Database.Database;

  afterEach(() => {
    db.close();
  });

  it("returns 200 with ok status when DB is healthy", async () => {
    db = createTestDb();
    const app = createApp(db);

    const res = await app.request("/health");

    expect(res.status).toBe(200);
    const body = (await res.json()) as { status: string; db: string };
    expect(body.status).toBe("ok");
    expect(body.db).toBe("connected");
  });

  it("returns 503 when DB is closed", async () => {
    db = createTestDb();
    const app = createApp(db);
    db.close();

    const res = await app.request("/health");

    expect(res.status).toBe(503);
    const body = (await res.json()) as { status: string; db: string };
    expect(body.status).toBe("error");
  });
});

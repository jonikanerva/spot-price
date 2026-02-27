import { describe, it, expect, afterEach } from "vitest";
import { initTestDatabase, closeDatabase } from "./db.js";
import type Database from "better-sqlite3";

interface TableInfo {
  name: string;
}

interface ColumnInfo {
  name: string;
  type: string;
  notnull: number;
  dflt_value: string | null;
  pk: number;
}

interface MigrationRecord {
  version: number;
  name: string;
}

const getTableNames = (db: Database.Database): readonly string[] => {
  const rows = db
    .prepare(
      "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .all() as readonly TableInfo[];
  return rows.map((r) => r.name);
};

const getColumns = (
  db: Database.Database,
  table: string,
): readonly ColumnInfo[] => {
  return db.prepare(`PRAGMA table_info(${table})`).all() as ColumnInfo[];
};

describe("migration system", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("creates all expected tables", () => {
    db = initTestDatabase();
    const tables = getTableNames(db);

    expect(tables).toContain("_migrations");
    expect(tables).toContain("prices");
    expect(tables).toContain("user_settings");
    expect(tables).toContain("api_keys");
    expect(tables).toContain("user");
    expect(tables).toContain("session");
    expect(tables).toContain("account");
    expect(tables).toContain("verification");
    expect(tables).toContain("usernames");
  });

  it("records migration versions", () => {
    db = initTestDatabase();
    const migrations = db
      .prepare("SELECT version, name FROM _migrations ORDER BY version")
      .all() as readonly MigrationRecord[];

    expect(migrations.length).toBe(5);
    expect(migrations[0]?.version).toBe(1);
    expect(migrations[0]?.name).toBe("create_prices");
    expect(migrations[1]?.version).toBe(2);
    expect(migrations[1]?.name).toBe("create_user_settings");
    expect(migrations[2]?.version).toBe(3);
    expect(migrations[2]?.name).toBe("create_api_keys");
    expect(migrations[3]?.version).toBe(4);
    expect(migrations[3]?.name).toBe("create_better_auth_tables");
    expect(migrations[4]?.version).toBe(5);
    expect(migrations[4]?.name).toBe("create_usernames");
  });

  it("is idempotent — running twice applies no extra migrations", async () => {
    db = initTestDatabase();

    // runMigrations already ran in initTestDatabase — import and run again
    const { runMigrations } = await import("./migrate.js");
    const result = runMigrations(db);

    expect(result.applied.length).toBe(0);
    expect(result.total).toBe(5);
  });
});

describe("prices table", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("has expected columns", () => {
    db = initTestDatabase();
    const columns = getColumns(db, "prices");
    const columnNames = columns.map((c) => c.name);

    expect(columnNames).toContain("id");
    expect(columnNames).toContain("delivery_start");
    expect(columnNames).toContain("delivery_end");
    expect(columnNames).toContain("price_eur_mwh");
    expect(columnNames).toContain("area");
    expect(columnNames).toContain("fetched_at");
  });

  it("enforces unique constraint on delivery_start + area", () => {
    db = initTestDatabase();

    db.prepare(
      "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES (?, ?, ?, ?)",
    ).run(
      "2026-02-24T00:00:00+02:00",
      "2026-02-24T01:00:00+02:00",
      45.23,
      "FI",
    );

    expect(() => {
      db.prepare(
        "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES (?, ?, ?, ?)",
      ).run(
        "2026-02-24T00:00:00+02:00",
        "2026-02-24T01:00:00+02:00",
        50.0,
        "FI",
      );
    }).toThrow();
  });

  it("allows same delivery_start for different areas", () => {
    db = initTestDatabase();

    db.prepare(
      "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES (?, ?, ?, ?)",
    ).run(
      "2026-02-24T00:00:00+02:00",
      "2026-02-24T01:00:00+02:00",
      45.23,
      "FI",
    );

    // Should not throw — different area
    db.prepare(
      "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES (?, ?, ?, ?)",
    ).run(
      "2026-02-24T00:00:00+02:00",
      "2026-02-24T01:00:00+02:00",
      38.1,
      "SE1",
    );

    const count = db.prepare("SELECT COUNT(*) as cnt FROM prices").get() as {
      cnt: number;
    };
    expect(count.cnt).toBe(2);
  });
});

describe("user_settings table", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("has correct defaults", () => {
    db = initTestDatabase();

    db.prepare("INSERT INTO user_settings (user_id) VALUES (?)").run(
      "test-user-1",
    );

    const row = db
      .prepare("SELECT * FROM user_settings WHERE user_id = ?")
      .get("test-user-1") as Record<string, unknown>;

    expect(row["margin_cents_kwh"]).toBe(0.49);
    expect(row["transfer_day_cents_kwh"]).toBe(2.92);
    expect(row["transfer_night_cents_kwh"]).toBe(1.37);
    expect(row["tax_cents_kwh"]).toBe(2.82752);
    expect(row["vat_percent"]).toBe(25.5);
    expect(row["night_start_hour"]).toBe(22);
    expect(row["night_end_hour"]).toBe(7);
    expect(row["timezone"]).toBe("Europe/Helsinki");
    expect(row["area"]).toBe("FI");
  });
});

describe("api_keys table", () => {
  let db: Database.Database;

  afterEach(() => {
    closeDatabase(db);
  });

  it("can store and retrieve an API key", () => {
    db = initTestDatabase();

    db.prepare(
      "INSERT INTO api_keys (id, user_id, key_hash, name) VALUES (?, ?, ?, ?)",
    ).run("key-1", "user-1", "hashed-value", "Home Assistant");

    const row = db
      .prepare("SELECT * FROM api_keys WHERE id = ?")
      .get("key-1") as Record<string, unknown>;

    expect(row["user_id"]).toBe("user-1");
    expect(row["key_hash"]).toBe("hashed-value");
    expect(row["name"]).toBe("Home Assistant");
    expect(row["created_at"]).toBeDefined();
    expect(row["last_used_at"]).toBeNull();
  });
});

import { describe, it, expect, afterEach } from "vitest";
import { initTestDatabase, closeDatabase } from "./db.js";
import type { Pool } from "pg";

interface TableInfo {
  name: string;
}

interface ColumnInfo {
  column_name: string;
  data_type: string;
  is_nullable: string;
  column_default: string | null;
}

interface MigrationRecord {
  version: number;
  name: string;
}

const getTableNames = async (pool: Pool): Promise<readonly string[]> => {
  const { rows } = await pool.query<TableInfo>(
    "SELECT tablename AS name FROM pg_tables WHERE schemaname = current_schema() ORDER BY tablename",
  );
  return rows.map((r) => r.name);
};

const getColumns = async (
  pool: Pool,
  table: string,
): Promise<readonly ColumnInfo[]> => {
  const { rows } = await pool.query<ColumnInfo>(
    "SELECT column_name, data_type, is_nullable, column_default FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = $1 ORDER BY ordinal_position",
    [table],
  );
  return rows;
};

describe("migration system", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("creates all expected tables", async () => {
    pool = await initTestDatabase();
    const tables = await getTableNames(pool);

    expect(tables).toContain("_migrations");
    expect(tables).toContain("prices");
    expect(tables).toContain("user_settings");
    expect(tables).toContain("api_keys");
    expect(tables).toContain("user");
    expect(tables).toContain("session");
    expect(tables).toContain("account");
    expect(tables).toContain("verification");
    expect(tables).toContain("usernames");
    expect(tables).toContain("fingrid_series");
  });

  it("records migration versions", async () => {
    pool = await initTestDatabase();
    const { rows: migrations } = await pool.query<MigrationRecord>(
      "SELECT version, name FROM _migrations ORDER BY version",
    );

    expect(migrations.length).toBe(3);
    expect(migrations[0]?.version).toBe(1);
    expect(migrations[0]?.name).toBe("baseline");
    expect(migrations[1]?.version).toBe(2);
    expect(migrations[1]?.name).toBe("fingrid_series");
    expect(migrations[2]?.version).toBe(3);
    expect(migrations[2]?.name).toBe("scrub_session_ip_ua");
  });

  it("is idempotent — running twice applies no extra migrations", async () => {
    pool = await initTestDatabase();

    const { runMigrations } = await import("./migrate.js");
    const result = await runMigrations(pool);

    expect(result.applied.length).toBe(0);
    expect(result.total).toBe(3);
  });
});

describe("prices table", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("has expected columns", async () => {
    pool = await initTestDatabase();
    const columns = await getColumns(pool, "prices");
    const columnNames = columns.map((c) => c.column_name);

    expect(columnNames).toContain("id");
    expect(columnNames).toContain("delivery_start");
    expect(columnNames).toContain("delivery_end");
    expect(columnNames).toContain("price_eur_mwh");
    expect(columnNames).toContain("area");
    expect(columnNames).toContain("fetched_at");
  });

  it("enforces unique constraint on delivery_start + area", async () => {
    pool = await initTestDatabase();

    await pool.query(
      "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES ($1, $2, $3, $4)",
      ["2026-02-24T00:00:00+02:00", "2026-02-24T01:00:00+02:00", 45.23, "FI"],
    );

    await expect(
      pool.query(
        "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES ($1, $2, $3, $4)",
        ["2026-02-24T00:00:00+02:00", "2026-02-24T01:00:00+02:00", 50.0, "FI"],
      ),
    ).rejects.toThrow();
  });

  it("allows same delivery_start for different areas", async () => {
    pool = await initTestDatabase();

    await pool.query(
      "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES ($1, $2, $3, $4)",
      ["2026-02-24T00:00:00+02:00", "2026-02-24T01:00:00+02:00", 45.23, "FI"],
    );

    await pool.query(
      "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) VALUES ($1, $2, $3, $4)",
      ["2026-02-24T00:00:00+02:00", "2026-02-24T01:00:00+02:00", 38.1, "SE1"],
    );

    const { rows } = await pool.query<{ cnt: string }>(
      "SELECT COUNT(*) AS cnt FROM prices",
    );
    expect(parseInt(rows[0]?.cnt ?? "0", 10)).toBe(2);
  });
});

describe("user_settings table", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("has correct defaults", async () => {
    pool = await initTestDatabase();

    await pool.query("INSERT INTO user_settings (user_id) VALUES ($1)", [
      "test-user-1",
    ]);

    const { rows } = await pool.query<Record<string, unknown>>(
      "SELECT * FROM user_settings WHERE user_id = $1",
      ["test-user-1"],
    );
    expect(rows).toHaveLength(1);
    const row = rows[0] as Record<string, unknown>;

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
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("can store and retrieve an API key", async () => {
    pool = await initTestDatabase();

    await pool.query(
      "INSERT INTO api_keys (id, user_id, key_plaintext) VALUES ($1, $2, $3)",
      ["key-1", "user-1", "sp_test_key_123"],
    );

    const { rows } = await pool.query<Record<string, unknown>>(
      "SELECT * FROM api_keys WHERE id = $1",
      ["key-1"],
    );
    expect(rows).toHaveLength(1);
    const row = rows[0] as Record<string, unknown>;

    expect(row["user_id"]).toBe("user-1");
    expect(row["key_plaintext"]).toBe("sp_test_key_123");
    expect(row["created_at"]).toBeDefined();
    expect(row["last_used_at"]).toBeNull();
  });
});

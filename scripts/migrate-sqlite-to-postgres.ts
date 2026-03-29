/**
 * Data migration script: SQLite → PostgreSQL
 *
 * Usage:
 *   SQLITE_PATH=/path/to/spot-price.db DATABASE_URL=postgresql://... npx tsx scripts/migrate-sqlite-to-postgres.ts
 *
 * This script:
 * 1. Opens the SQLite database (read-only)
 * 2. Connects to PostgreSQL (must already have the baseline migration applied)
 * 3. Migrates all data table-by-table in dependency order
 * 4. Converts SQLite types to PostgreSQL types (booleans, timestamps)
 * 5. Verifies row counts match
 */

import Database from "better-sqlite3";
import pg from "pg";

const { Pool } = pg;

const BATCH_SIZE = 1000;

const getSqlitePath = (): string => {
  const p = process.env["SQLITE_PATH"];
  if (!p) {
    throw new Error("SQLITE_PATH environment variable is required");
  }
  return p;
};

const getPostgresUrl = (): string => {
  const url = process.env["DATABASE_URL"];
  if (!url) {
    throw new Error("DATABASE_URL environment variable is required");
  }
  return url;
};

/** Normalize SQLite datetime('now') format "YYYY-MM-DD HH:MM:SS" to ISO 8601 */
const normalizeTimestamp = (val: string | null): string | null => {
  if (!val) return null;
  // Already ISO 8601 with T and Z
  if (val.includes("T")) return val;
  // SQLite datetime('now') format: "YYYY-MM-DD HH:MM:SS"
  return `${val.replace(" ", "T")}Z`;
};

interface TableMigration {
  readonly name: string;
  readonly selectSql: string;
  readonly insertSql: string;
  readonly transformRow: (row: Record<string, unknown>) => unknown[];
}

const migrations: readonly TableMigration[] = [
  // 1. user (no foreign key deps)
  {
    name: "user",
    selectSql: 'SELECT * FROM "user"',
    insertSql: `INSERT INTO "user" ("id", "name", "email", "emailVerified", "image", "createdAt", "updatedAt")
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT ("id") DO NOTHING`,
    transformRow: (r) => [
      r["id"],
      r["name"],
      r["email"],
      r["emailVerified"] === 1 || r["emailVerified"] === true,
      r["image"] ?? null,
      normalizeTimestamp(r["createdAt"] as string),
      normalizeTimestamp(r["updatedAt"] as string),
    ],
  },
  // 2. account (depends on user)
  {
    name: "account",
    selectSql: 'SELECT * FROM "account"',
    insertSql: `INSERT INTO "account" ("id", "accountId", "providerId", "userId", "accessToken", "refreshToken",
                "idToken", "accessTokenExpiresAt", "refreshTokenExpiresAt", "scope", "password", "createdAt", "updatedAt")
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT ("id") DO NOTHING`,
    transformRow: (r) => [
      r["id"],
      r["accountId"],
      r["providerId"],
      r["userId"],
      r["accessToken"] ?? null,
      r["refreshToken"] ?? null,
      r["idToken"] ?? null,
      normalizeTimestamp(r["accessTokenExpiresAt"] as string | null),
      normalizeTimestamp(r["refreshTokenExpiresAt"] as string | null),
      r["scope"] ?? null,
      r["password"] ?? null,
      normalizeTimestamp(r["createdAt"] as string),
      normalizeTimestamp(r["updatedAt"] as string),
    ],
  },
  // 3. session (depends on user)
  {
    name: "session",
    selectSql: 'SELECT * FROM "session"',
    insertSql: `INSERT INTO "session" ("id", "expiresAt", "token", "createdAt", "updatedAt", "ipAddress", "userAgent", "userId")
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT ("id") DO NOTHING`,
    transformRow: (r) => [
      r["id"],
      normalizeTimestamp(r["expiresAt"] as string),
      r["token"],
      normalizeTimestamp(r["createdAt"] as string),
      normalizeTimestamp(r["updatedAt"] as string),
      r["ipAddress"] ?? null,
      r["userAgent"] ?? null,
      r["userId"],
    ],
  },
  // 4. verification (no deps)
  {
    name: "verification",
    selectSql: 'SELECT * FROM "verification"',
    insertSql: `INSERT INTO "verification" ("id", "identifier", "value", "expiresAt", "createdAt", "updatedAt")
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT ("id") DO NOTHING`,
    transformRow: (r) => [
      r["id"],
      r["identifier"],
      r["value"],
      normalizeTimestamp(r["expiresAt"] as string),
      normalizeTimestamp(r["createdAt"] as string),
      normalizeTimestamp(r["updatedAt"] as string),
    ],
  },
  // 5. usernames (depends on user)
  {
    name: "usernames",
    selectSql: "SELECT * FROM usernames",
    insertSql: `INSERT INTO usernames (user_id, username, created_at)
                VALUES ($1, $2, $3)
                ON CONFLICT (user_id) DO NOTHING`,
    transformRow: (r) => [
      r["user_id"],
      r["username"],
      normalizeTimestamp(r["created_at"] as string),
    ],
  },
  // 6. user_settings (depends on user conceptually)
  {
    name: "user_settings",
    selectSql: "SELECT * FROM user_settings",
    insertSql: `INSERT INTO user_settings (user_id, margin_cents_kwh, transfer_day_cents_kwh,
                transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour,
                night_end_hour, timezone, area, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (user_id) DO NOTHING`,
    transformRow: (r) => [
      r["user_id"],
      r["margin_cents_kwh"],
      r["transfer_day_cents_kwh"],
      r["transfer_night_cents_kwh"],
      r["tax_cents_kwh"],
      r["vat_percent"],
      r["night_start_hour"],
      r["night_end_hour"],
      r["timezone"],
      r["area"],
      normalizeTimestamp(r["updated_at"] as string),
    ],
  },
  // 7. api_keys
  {
    name: "api_keys",
    selectSql: "SELECT * FROM api_keys",
    insertSql: `INSERT INTO api_keys (id, user_id, key_plaintext, created_at, last_used_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (id) DO NOTHING`,
    transformRow: (r) => [
      r["id"],
      r["user_id"],
      r["key_plaintext"],
      normalizeTimestamp(r["created_at"] as string),
      normalizeTimestamp(r["last_used_at"] as string | null),
    ],
  },
  // 8. prices (largest table, last)
  {
    name: "prices",
    selectSql:
      "SELECT delivery_start, delivery_end, price_eur_mwh, area, fetched_at FROM prices",
    insertSql: `INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area, fetched_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (delivery_start, area) DO NOTHING`,
    transformRow: (r) => [
      normalizeTimestamp(r["delivery_start"] as string),
      normalizeTimestamp(r["delivery_end"] as string),
      r["price_eur_mwh"],
      r["area"],
      normalizeTimestamp(r["fetched_at"] as string),
    ],
  },
];

const migrateTable = async (
  sqliteDb: Database.Database,
  pool: pg.Pool,
  table: TableMigration,
): Promise<{ name: string; sqliteCount: number; pgCount: number }> => {
  const rows = sqliteDb
    .prepare(table.selectSql)
    .all() as Record<string, unknown>[];
  const sqliteCount = rows.length;

  if (sqliteCount === 0) {
    console.log(`  ${table.name}: 0 rows (empty)`);
    const { rows: pgRows } = await pool.query<{ cnt: string }>(
      `SELECT COUNT(*) as cnt FROM "${table.name}"`,
    );
    return { name: table.name, sqliteCount: 0, pgCount: parseInt(pgRows[0]?.cnt ?? "0", 10) };
  }

  // Batch insert
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    for (let i = 0; i < rows.length; i += BATCH_SIZE) {
      const batch = rows.slice(i, i + BATCH_SIZE);
      for (const row of batch) {
        const params = table.transformRow(row);
        await client.query(table.insertSql, params);
      }
    }
    await client.query("COMMIT");
  } catch (err) {
    await client.query("ROLLBACK");
    throw err;
  } finally {
    client.release();
  }

  const { rows: pgRows } = await pool.query<{ cnt: string }>(
    `SELECT COUNT(*) as cnt FROM "${table.name}"`,
  );
  const pgCount = parseInt(pgRows[0]?.cnt ?? "0", 10);

  const status = pgCount >= sqliteCount ? "OK" : "MISMATCH";
  console.log(
    `  ${table.name}: ${String(sqliteCount)} SQLite → ${String(pgCount)} PG [${status}]`,
  );

  return { name: table.name, sqliteCount, pgCount };
};

const main = async (): Promise<void> => {
  const sqlitePath = getSqlitePath();
  const postgresUrl = getPostgresUrl();

  console.log(`SQLite: ${sqlitePath}`);
  console.log(`PostgreSQL: ${postgresUrl.replace(/:[^:@]+@/, ":***@")}`);
  console.log("");

  const sqliteDb = new Database(sqlitePath, { readonly: true });
  const pool = new Pool({ connectionString: postgresUrl });

  try {
    console.log("Migrating tables...");
    const results = [];
    for (const table of migrations) {
      const result = await migrateTable(sqliteDb, pool, table);
      results.push(result);
    }

    console.log("\n--- Summary ---");
    let allOk = true;
    for (const r of results) {
      const ok = r.pgCount >= r.sqliteCount;
      if (!ok) allOk = false;
      console.log(
        `  ${r.name}: ${String(r.sqliteCount)} → ${String(r.pgCount)} ${ok ? "✓" : "✗ MISMATCH"}`,
      );
    }

    if (allOk) {
      console.log("\nMigration complete — all tables verified.");
    } else {
      console.error("\nWARNING: Some tables have mismatched row counts!");
      process.exit(1);
    }
  } finally {
    sqliteDb.close();
    await pool.end();
  }
};

void main();

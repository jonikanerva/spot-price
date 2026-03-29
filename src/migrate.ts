import type { Pool } from "pg";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

interface MigrationRecord {
  version: number;
  name: string;
  applied_at: string;
}

const MIGRATIONS_DIR = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "migrations",
);

const ensureMigrationsTable = async (pool: Pool): Promise<void> => {
  await pool.query(`
    CREATE TABLE IF NOT EXISTS _migrations (
      version INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    )
  `);
};

const getAppliedVersions = async (
  pool: Pool,
): Promise<ReadonlySet<number>> => {
  const { rows } = await pool.query<MigrationRecord>(
    "SELECT version FROM _migrations ORDER BY version",
  );
  return new Set(rows.map((r) => r.version));
};

interface MigrationFile {
  readonly version: number;
  readonly name: string;
  readonly filePath: string;
}

const discoverMigrations = (): readonly MigrationFile[] => {
  const files = readdirSync(MIGRATIONS_DIR).filter((f) => f.endsWith(".sql"));

  return files
    .map((fileName): MigrationFile | null => {
      // Expected format: 001_create_prices.sql
      const match = /^(\d+)_(.+)\.sql$/.exec(fileName);
      if (!match?.[1] || !match[2]) {
        return null;
      }
      return {
        version: parseInt(match[1], 10),
        name: match[2],
        filePath: path.join(MIGRATIONS_DIR, fileName),
      };
    })
    .filter((m): m is MigrationFile => m !== null)
    .toSorted((a, b) => a.version - b.version);
};

export interface MigrationResult {
  readonly applied: readonly string[];
  readonly total: number;
}

export const runMigrations = async (pool: Pool): Promise<MigrationResult> => {
  await ensureMigrationsTable(pool);

  const applied = await getAppliedVersions(pool);
  const migrations = discoverMigrations();
  const pending = migrations.filter((m) => !applied.has(m.version));

  const appliedNames: string[] = [];

  for (const migration of pending) {
    const sql = readFileSync(migration.filePath, "utf-8");
    const client = await pool.connect();

    try {
      await client.query("BEGIN");
      await client.query(sql);
      await client.query(
        "INSERT INTO _migrations (version, name) VALUES ($1, $2)",
        [migration.version, migration.name],
      );
      await client.query("COMMIT");
    } catch (err) {
      await client.query("ROLLBACK");
      throw err;
    } finally {
      client.release();
    }

    const label = `${String(migration.version).padStart(3, "0")}_${migration.name}`;
    appliedNames.push(label);
    console.log(`Migration applied: ${label}`);
  }

  return {
    applied: appliedNames,
    total: migrations.length,
  };
};

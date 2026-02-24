import type Database from "better-sqlite3";
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

const ensureMigrationsTable = (db: Database.Database): void => {
  db.exec(`
    CREATE TABLE IF NOT EXISTS _migrations (
      version INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      applied_at TEXT NOT NULL DEFAULT (datetime('now'))
    )
  `);
};

const getAppliedVersions = (db: Database.Database): ReadonlySet<number> => {
  const rows = db
    .prepare("SELECT version FROM _migrations ORDER BY version")
    .all() as readonly MigrationRecord[];
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

export const runMigrations = (db: Database.Database): MigrationResult => {
  ensureMigrationsTable(db);

  const applied = getAppliedVersions(db);
  const migrations = discoverMigrations();
  const pending = migrations.filter((m) => !applied.has(m.version));

  const appliedNames: string[] = [];

  for (const migration of pending) {
    const sql = readFileSync(migration.filePath, "utf-8");

    db.transaction(() => {
      db.exec(sql);
      db.prepare("INSERT INTO _migrations (version, name) VALUES (?, ?)").run(
        migration.version,
        migration.name,
      );
    })();

    appliedNames.push(
      `${String(migration.version).padStart(3, "0")}_${migration.name}`,
    );
    console.log(
      `Migration applied: ${String(migration.version).padStart(3, "0")}_${migration.name}`,
    );
  }

  return {
    applied: appliedNames,
    total: migrations.length,
  };
};

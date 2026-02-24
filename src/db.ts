import Database from "better-sqlite3";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { runMigrations } from "./migrate.js";

const DEFAULT_DB_DIR = "data";
const DEFAULT_DB_NAME = "spot-price.db";

const getDbPath = (): string => {
  const envPath = process.env["DATABASE_PATH"];
  if (envPath) {
    return envPath;
  }
  return path.join(DEFAULT_DB_DIR, DEFAULT_DB_NAME);
};

const ensureDirectory = (filePath: string): void => {
  const dir = path.dirname(filePath);
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
};

const configurePragmas = (db: Database.Database): void => {
  // Enable WAL mode for concurrent reads during writes
  db.pragma("journal_mode = WAL");
  // Enforce foreign key constraints
  db.pragma("foreign_keys = ON");
  // Recommended for WAL mode performance
  db.pragma("synchronous = NORMAL");
};

export const initDatabase = (): Database.Database => {
  const dbPath = getDbPath();
  ensureDirectory(dbPath);

  const db = new Database(dbPath);
  configurePragmas(db);

  const result = runMigrations(db);
  if (result.applied.length > 0) {
    console.log(
      `Migrations: ${String(result.applied.length)} applied, ${String(result.total)} total`,
    );
  }

  return db;
};

/** Create an in-memory database for testing (with migrations applied) */
export const initTestDatabase = (): Database.Database => {
  const db = new Database(":memory:");
  configurePragmas(db);
  runMigrations(db);
  return db;
};

export const closeDatabase = (db: Database.Database): void => {
  db.close();
};

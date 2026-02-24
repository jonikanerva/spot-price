import Database from "better-sqlite3";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";

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

export const initDatabase = (): Database.Database => {
  const dbPath = getDbPath();
  ensureDirectory(dbPath);

  const db = new Database(dbPath);

  // Enable WAL mode for concurrent reads during writes
  db.pragma("journal_mode = WAL");
  // Enforce foreign key constraints
  db.pragma("foreign_keys = ON");
  // Recommended for WAL mode performance
  db.pragma("synchronous = NORMAL");

  return db;
};

export const closeDatabase = (db: Database.Database): void => {
  db.close();
};

import Database from "better-sqlite3";
import { betterAuth } from "better-auth";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";

const DEFAULT_DB_PATH = path.join("data", "spot-price.db");

const getDatabasePath = (): string =>
  process.env["DATABASE_PATH"] ?? DEFAULT_DB_PATH;

const ensureDatabaseDirectory = (dbPath: string): void => {
  const dir = path.dirname(dbPath);
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
};

const dbPath = getDatabasePath();
ensureDatabaseDirectory(dbPath);
const authDb = new Database(dbPath);

export const auth = betterAuth({
  baseURL: process.env["BETTER_AUTH_URL"] ?? "http://localhost:3000",
  secret:
    process.env["BETTER_AUTH_SECRET"] ??
    "dev-only-better-auth-secret-must-be-32+",
  database: authDb,
  emailAndPassword: {
    enabled: true,
    autoSignIn: true,
  },
});

import pg from "pg";
import { runMigrations } from "./migrate.js";

const { Pool, types } = pg;
export type { Pool } from "pg";

// Return TIMESTAMPTZ (OID 1184) as ISO 8601 UTC strings instead of Date objects
// so the rest of the application code remains unchanged.
types.setTypeParser(1184, (val: string) => new Date(val).toISOString());
// TIMESTAMP without tz (OID 1114) — treat as UTC
types.setTypeParser(1114, (val: string) => new Date(val + "Z").toISOString());

const getConnectionString = (): string => {
  const url = process.env["DATABASE_URL"];
  if (!url) {
    throw new Error("DATABASE_URL environment variable is required");
  }
  return url;
};

export const initDatabase = async (): Promise<pg.Pool> => {
  const pool = new Pool({ connectionString: getConnectionString() });

  const result = await runMigrations(pool);
  if (result.applied.length > 0) {
    console.log(
      `Migrations: ${String(result.applied.length)} applied, ${String(result.total)} total`,
    );
  }

  return pool;
};

/** WeakMap to track test schema names for cleanup without monkey-patching Pool */
const testSchemas = new WeakMap<pg.Pool, string>();

/** Create a test database using a unique schema for isolation */
export const initTestDatabase = async (): Promise<pg.Pool> => {
  const connectionString =
    process.env["TEST_DATABASE_URL"] ?? process.env["DATABASE_URL"];
  if (!connectionString) {
    throw new Error(
      "TEST_DATABASE_URL or DATABASE_URL environment variable is required for tests",
    );
  }

  // Schema name uses only [a-z0-9_] so it is safe for both SQL identifiers
  // and the -c search_path= connection option without additional escaping.
  const schemaName = `test_${String(Date.now())}_${Math.random().toString(36).slice(2, 8)}`;
  if (!/^[a-z0-9_]+$/.test(schemaName)) {
    throw new Error(`Unsafe test schema name: ${schemaName}`);
  }

  // Pass search_path via the PostgreSQL options connection parameter so every
  // connection automatically uses the test schema without a connect listener.
  const pool = new Pool({
    connectionString,
    options: `-c search_path=${schemaName}`,
  });

  await pool.query(`CREATE SCHEMA ${schemaName}`);

  await runMigrations(pool);

  testSchemas.set(pool, schemaName);

  return pool;
};

export const closeDatabase = async (pool: pg.Pool): Promise<void> => {
  const testSchema = testSchemas.get(pool);
  if (testSchema) {
    await pool.query(`DROP SCHEMA IF EXISTS ${testSchema} CASCADE`);
  }
  await pool.end();
};

import { fileURLToPath } from "node:url";
import pg from "pg";

/**
 * The DEDICATED end-to-end database. Hard-coded literal (NOT read from any env
 * or `src/env.ts`/`src/db.ts`) so it can never resolve to the dev `spot_price`
 * database and wipe real data. `playwright.config.ts` imports this same constant
 * for `webServer.env.DATABASE_URL`, so the reset target and the app under test
 * cannot drift apart.
 */
export const E2E_DATABASE_URL =
  "postgresql://spot:spot@localhost:5432/spot_price_e2e";

/** Maintenance DB used only to CREATE the e2e DB when it is missing. */
const MAINTENANCE_DATABASE_URL =
  "postgresql://spot:spot@localhost:5432/postgres";

/** The exact database name the guard requires before any destructive SQL. */
const E2E_DATABASE_NAME = "spot_price_e2e";

/**
 * Reset the dedicated e2e database before the suite runs. Imports NOTHING from
 * `src/` — a bare `pg.Client` against the literal above — so the dev
 * `DATABASE_URL` is never in scope and the wipe path that the SQLite version
 * left open is closed on both surfaces (here, and the app via webServer.env).
 *
 * Steps: create the e2e DB if absent (CREATE only — never DROP a database),
 * connect to it, GUARD on `current_database()`, then drop+recreate the `public`
 * schema, leaving an empty schema.
 *
 * This runs as a `test:e2e` PRE-STEP (`tsx e2e/global-setup.ts`) rather than
 * Playwright's `globalSetup` hook: Playwright does not guarantee `globalSetup`
 * finishes before the `webServer` boots, and the server migrates on startup —
 * so a hook-ordered reset could drop the schema the server just created, racing
 * to an empty DB. Running the reset before Playwright launches makes the order
 * deterministic: reset → empty schema → Playwright starts the server → the
 * server's startup migration (`initDatabase` → `runMigrations`) populates the
 * clean schema. `smoke.spec.ts`'s fresh-user-per-run logic relies on this slate.
 */
export default async function globalSetup(): Promise<void> {
  await ensureE2eDatabaseExists();

  const client = new pg.Client({ connectionString: E2E_DATABASE_URL });
  try {
    await client.connect();

    // Belt-and-braces: turn any misconfiguration into a loud failure instead of
    // a destructive operation against the wrong database.
    const { rows } = await client.query<{ current_database: string }>(
      "SELECT current_database()",
    );
    const current = rows[0]?.current_database;
    if (current !== E2E_DATABASE_NAME) {
      throw new Error(
        `E2E reset refused: connected to "${String(current)}", expected "${E2E_DATABASE_NAME}". ` +
          "Aborting before any destructive SQL to protect the dev database.",
      );
    }

    await client.query("DROP SCHEMA public CASCADE");
    await client.query("CREATE SCHEMA public");
  } finally {
    await client.end();
  }
}

/**
 * Connect to the e2e DB; if it is missing, create it via the maintenance DB.
 * CREATE-only — this never drops a database.
 */
const ensureE2eDatabaseExists = async (): Promise<void> => {
  const probe = new pg.Client({ connectionString: E2E_DATABASE_URL });
  try {
    await probe.connect();
    // It exists — nothing to create.
    await probe.end();
    return;
  } catch {
    // Probe failed (most likely the DB does not exist yet); fall through to
    // create it. Ensure the probe socket is closed regardless.
    try {
      await probe.end();
    } catch {
      // Already closed / never opened — ignore.
    }
  }

  const admin = new pg.Client({ connectionString: MAINTENANCE_DATABASE_URL });
  try {
    await admin.connect();
    // CREATE only. The identifier is a fixed literal, not user input.
    await admin.query(`CREATE DATABASE ${E2E_DATABASE_NAME}`);
  } catch (error) {
    // A concurrent run may have created it between the probe and here; tolerate
    // "already exists" and rethrow anything else.
    const message = error instanceof Error ? error.message : String(error);
    if (!message.includes("already exists")) {
      throw error;
    }
  } finally {
    await admin.end();
  }
};

// Run as a script (`pnpm tsx e2e/global-setup.ts`, the test:e2e pre-step). The
// default export above also lets Playwright call it as a globalSetup hook if
// ever wired that way, but the deterministic path is the pre-step.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  globalSetup().catch((error: unknown) => {
    console.error(error);
    process.exit(1);
  });
}

import { unlinkSync } from "node:fs";

const E2E_DB = "data/test-e2e.db";

/** Remove stale E2E database so each test run starts fresh. */
export default function globalSetup(): void {
  for (const suffix of ["", "-shm", "-wal"]) {
    try {
      unlinkSync(`${E2E_DB}${suffix}`);
    } catch {
      // File doesn't exist — nothing to clean
    }
  }
}

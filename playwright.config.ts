import { defineConfig } from "@playwright/test";
import { E2E_DATABASE_URL } from "./e2e/global-setup.js";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  retries: 0,
  // The e2e DB reset runs as a `test:e2e` PRE-STEP (`tsx e2e/global-setup.ts`),
  // NOT a Playwright globalSetup hook: Playwright does not guarantee globalSetup
  // completes before the webServer boots and migrates, so a hook-ordered reset
  // could drop the schema the server just created. Resetting before Playwright
  // launches keeps the order deterministic. The `E2E_DATABASE_URL` import below
  // keeps the reset target and the app-under-test pointed at the same DB.
  use: {
    baseURL: "http://localhost:3000",
    headless: true,
  },
  webServer: {
    command: "pnpm tsx src/index.ts",
    port: 3000,
    timeout: 15_000,
    reuseExistingServer: false,
    // `webServer.env` is a whitelist that takes precedence in the child
    // process, so DATABASE_URL here OVERRIDES any ambient dev DATABASE_URL —
    // the app under test always points at the dedicated e2e DB, never dev data.
    env: {
      PORT: "3000",
      DATABASE_URL: E2E_DATABASE_URL,
      NODE_ENV: "test",
      BETTER_AUTH_SECRET: "e2e-test-secret-key-do-not-use-in-prod",
    },
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});

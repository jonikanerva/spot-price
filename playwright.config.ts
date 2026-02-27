import { defineConfig } from "@playwright/test";

const E2E_DB = "data/test-e2e.db";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  retries: 0,
  globalSetup: "./e2e/global-setup.ts",
  use: {
    baseURL: "http://localhost:3000",
    headless: true,
  },
  webServer: {
    command: "pnpm tsx src/index.ts",
    port: 3000,
    timeout: 15_000,
    reuseExistingServer: false,
    env: {
      PORT: "3000",
      DATABASE_PATH: E2E_DB,
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

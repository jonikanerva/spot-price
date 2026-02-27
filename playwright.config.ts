import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  retries: 0,
  use: {
    baseURL: "http://localhost:3000",
    headless: true,
  },
  webServer: {
    command: "pnpm tsx src/index.ts",
    port: 3000,
    timeout: 15_000,
    reuseExistingServer: true,
    env: {
      PORT: "3000",
      DATABASE_PATH: "data/test-e2e.db",
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

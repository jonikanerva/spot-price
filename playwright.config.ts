// The browser journeys of `mise run e2e`.
//
// One server runs a copy of the release binary (`spot-price serve`): the
// production router, middleware, session rules, and jobs. `mise run e2e`
// builds the binary, copies it to the run directory, creates the database of
// `E2E_DATABASE_URL` empty on the test server of the run, migrates it, and
// seeds it with e2e/seed.sql. Run the journeys only through that task:
// without the seed, the start-up price fetch would call Nord Pool.
import { defineConfig } from "@playwright/test";

// `scripts/db.sh e2e` sets these values for each run, so two runs never
// share a server port, a database, a binary, or an output directory.
const required = (name: string): string => {
  const value = process.env[name] ?? "";
  if (value === "") {
    throw new Error(
      `${name} is not set; run the journeys with \`mise run e2e\`.`,
    );
  }
  return value;
};
const SERVER = required("E2E_SERVER");
const PORT = Number(required("E2E_PORT"));
const OUTPUT_DIR = required("E2E_OUTPUT_DIR");
// The server compares the `Origin` header of a state change byte for byte
// with the origin of BETTER_AUTH_URL, so both use this exact value.
const BASE_URL = `http://127.0.0.1:${String(PORT)}`;

// The journeys reset and fill this database. Only a database on the local
// test server of this run is accepted.
const DATABASE_URL = required("E2E_DATABASE_URL");
const TEST_SERVER = required("SPOT_PRICE_DB_CONTAINER");
if (
  !/^postgres:\/\/postgres:[0-9a-f]+@127\.0\.0\.1:\d+\/spot_price_e2e$/.test(
    DATABASE_URL,
  ) ||
  !TEST_SERVER.startsWith("spot-price-test-")
) {
  throw new Error(
    "E2E_DATABASE_URL must name spot_price_e2e on a local test server; run the journeys with `mise run e2e`.",
  );
}

// Every outbound HTTP request of the server goes to this closed local port,
// so a missed seed makes an upstream fetch fail instead of reaching a live
// API. The inbound port and PostgreSQL do not use the proxy.
const DEAD_PROXY = "http://127.0.0.1:9";

export default defineConfig({
  testDir: "./e2e",
  outputDir: OUTPUT_DIR,
  forbidOnly: true,
  timeout: 30_000,
  // A failure is investigated, never retried away (DOCTRINE.md P6).
  retries: 0,
  // The journeys sign in 10 times. The server allows one client address 10
  // sign-ins per 15 minutes, so a new sign-in needs a new server process.
  workers: 1,
  reporter: "list",
  use: {
    baseURL: BASE_URL,
    headless: true,
    trace: "retain-on-failure",
  },
  webServer: {
    // `env -u`: a key in the shell must not turn on the Fingrid or the
    // weather job, which call live upstreams.
    command: `env -u FINGRID_API_KEY -u OPENWEATHERMAP_API_KEY "${SERVER}" serve`,
    url: `${BASE_URL}/health`,
    timeout: 30_000,
    reuseExistingServer: false,
    // The server log shows that each price run skipped the Nord Pool call.
    stdout: "pipe",
    stderr: "pipe",
    gracefulShutdown: { signal: "SIGTERM", timeout: 15_000 },
    // Playwright adds these values to the environment of the shell; a value
    // here wins over the shell.
    env: {
      NODE_ENV: "test",
      PORT: String(PORT),
      BETTER_AUTH_URL: BASE_URL,
      DATABASE_URL,
      LOG_FORMAT: "text",
      HTTPS_PROXY: DEAD_PROXY,
      https_proxy: DEAD_PROXY,
      HTTP_PROXY: DEAD_PROXY,
      http_proxy: DEAD_PROXY,
      ALL_PROXY: DEAD_PROXY,
      all_proxy: DEAD_PROXY,
      NO_PROXY: "",
      no_proxy: "",
    },
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});

import { z } from "zod";

/**
 * Centralised environment-variable parsing and validation.
 *
 * All non-test source modules read environment variables through `loadEnv()`
 * rather than touching `process.env` directly. Test files (`*.test.ts`,
 * `src/test-utils.ts`) are exempt because they mutate `process.env` per test
 * to exercise different branches; everywhere else, `process.env` is off
 * limits — this is the rule declared in `STACK.md §7`.
 *
 * `loadEnv()` is intentionally a *function*, not a module-level constant:
 * tests need to mutate `process.env` between calls (e.g. flipping
 * `NODE_ENV=production` to assert that startup requires
 * `BETTER_AUTH_SECRET`), and a frozen constant would capture the value at
 * import time and make those tests impossible. The cost is a tiny extra
 * Zod parse on each call; the benefit is that the rule stays a single
 * code path with no escape hatches.
 *
 * Production fail-fast is handled by `index.ts` calling `loadEnv()` once
 * at the top of `main()` before any service is constructed — if validation
 * fails the process exits with a clear error, before the HTTP server ever
 * starts listening.
 */

const baseSchema = z.object({
  /** Node runtime mode. Drives the production-required env checks below. */
  NODE_ENV: z
    .enum(["development", "test", "production"])
    .default("development"),

  /** PostgreSQL connection string. Required outside test mode. */
  DATABASE_URL: z.string().min(1).optional(),

  /**
   * Public PostgreSQL connection string for DEV TOOLING ONLY (e.g. the offline
   * `pnpm backtest --db` CLI run from a developer machine against the deployed
   * DB's public endpoint). Optional and intentionally NOT in the production
   * `requireFor` checks — the server never depends on it; it reads
   * `DATABASE_URL`. The backtest CLI prefers this when set, falling back to
   * `DATABASE_URL`.
   */
  DATABASE_PUBLIC_URL: z.string().min(1).optional(),

  /**
   * Optional override used by the Vitest test setup so unit/integration
   * tests can hit a separate PostgreSQL database from the dev one. Falls
   * back to `DATABASE_URL` in `initTestDatabase()` when not set.
   */
  TEST_DATABASE_URL: z.string().min(1).optional(),

  /** HTTP listen port. Defaults to 3000; clamped to a valid TCP range. */
  PORT: z.coerce.number().int().min(1).max(65535).default(3000),

  /** Public base URL of the deployed auth surface. Required in production. */
  BETTER_AUTH_URL: z.url().optional(),

  /** Secret used by Better Auth to sign sessions. Required in production. */
  BETTER_AUTH_SECRET: z.string().min(1).optional(),

  /**
   * Fingrid Open Data API key, used by the FI price-forecast feature to fetch
   * public wind/consumption grid series (`data.fingrid.fi`). Optional: when
   * absent the forecast cron and endpoint degrade gracefully (the endpoint
   * returns `available: false`). It is never required for the authoritative
   * Nord Pool price path. A loud one-time startup warning fires in production
   * when it is missing (see `index.ts`), so the disabled state is visible in
   * Railway logs rather than hidden behind a 200 response.
   */
  FINGRID_API_KEY: z.string().min(1).optional(),
});

export type Env = z.infer<typeof baseSchema>;

const requireFor = (
  parsed: Env,
  key: "DATABASE_URL" | "BETTER_AUTH_URL" | "BETTER_AUTH_SECRET",
  context: string,
): void => {
  if (!parsed[key] || parsed[key].trim().length === 0) {
    throw new Error(`${key} is required ${context}`);
  }
};

const parseEnv = (raw: NodeJS.ProcessEnv): Env => {
  const result = baseSchema.safeParse(raw);
  if (!result.success) {
    const issues = result.error.issues
      .map((issue) => `${issue.path.join(".")}: ${issue.message}`)
      .join("; ");
    throw new Error(`Invalid environment configuration: ${issues}`);
  }
  const parsed = result.data;

  if (parsed.NODE_ENV === "production") {
    requireFor(parsed, "DATABASE_URL", "in production");
    requireFor(parsed, "BETTER_AUTH_URL", "in production");
    requireFor(parsed, "BETTER_AUTH_SECRET", "in production");
  }

  return parsed;
};

/**
 * Read and validate `process.env` against the schema above. Throws a single
 * `Error` describing every failure if anything is missing or malformed.
 *
 * Call this from non-test source code wherever environment variables are
 * needed. Do not cache the result across configuration changes — tests
 * that flip `NODE_ENV` per-case depend on each call seeing the current
 * `process.env` state.
 */
export const loadEnv = (): Env => parseEnv(process.env);

# STACK.md — Strict TypeScript / Node 24 LTS / Hono profile

> Single-package Node 24 + Hono + TypeScript backend for the spot-price API. The web UI is a small set of server-rendered HTML strings emitted from Hono routes (`src/ui.ts`) plus a single inline client script (`src/ui-client.ts`) — there is no React, no Vite, no SPA, no monorepo. All source lives under `src/` and is built with `tsup`; the project is deployed as a single Node process on Railway.

---

## 0. Project shape

- **Shape:** single-package backend service. One Hono process exposes the REST API (`/api/v1/...`), a setup-only server-rendered HTML UI (`src/ui.ts` + inline `src/ui-client.ts`), the OpenAPI 3.1 document (`/api/v1/openapi.json`) with the interactive Scalar reference (`/api/docs`), and an in-process `node-cron` price-fetch job. **No `apps/`, no `packages/`, no frontend build.**
- **Offline dev tooling:** the forecast backtest engine/CLI, `regenerate-bands`, and `backtest-metrics` live under `tools/` (not `src/`), are excluded from the production bundle (tsup's only entry is `src/index.ts`, and an ESLint guard forbids `src/` runtime from importing `tools/`), and are run on demand via `pnpm backtest` / `pnpm tsx tools/regenerate-bands.ts` — never in the server process.
- **Critical execution path:** the per-request hot path on the API (price-now / today / tomorrow / cheapest / history / forecast) and the day-ahead price fetch path (`src/fetch-job.ts`). Price math is a pure function of `(HourlyPrice, UserSettings)` in `src/calculator.ts`; the forecast estimate is a pure function of its inputs in `src/forecast.ts` (no DB, clock, or network) — the forecast route reads pre-fetched Fingrid rows off the DB and never calls Fingrid synchronously.
- **Applicable states:** API responses are typed success / typed error. When Nord Pool has not published, the answer is an explicit `available: false` / `404` — never an estimate (`VISION.md → One trusted upstream`). The setup UI handles awaiting-first-data, success, empty, permission/auth-blocked, and error.

---

## 1. Language & Runtime

- **Primary language:** TypeScript 5.9 (latest stable; do not jump to a 6.x prerelease)
- **Strictness mode:** `"strict": true`, `"noUncheckedIndexedAccess": true`, `"exactOptionalPropertyTypes": true`, `"noImplicitOverride": true`, `"verbatimModuleSyntax": true`, plus `"noUnusedLocals"`, `"noUnusedParameters"`, `"noFallthroughCasesInSwitch"`, `"isolatedModules"`. ESLint with `@typescript-eslint/strict-type-checked` (flat config).
- **Target runtime:** Node.js 24 LTS (Krypton)
- **Minimum runtime version:** `>= 24.15.0` (current LTS patch, pinned in `.nvmrc`). No back-deployment to Node 22 or earlier.
- **Package manager:** pnpm 10 (single package — **no workspaces**, no `apps/`, no `packages/`)
- **Lockfile:** `pnpm-lock.yaml`

---

## 2. Frameworks

| Concern                | Framework / library                                                                                      | Notes                                                                                                                   |
| ---------------------- | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Backend HTTP framework | Hono + `@hono/node-server`                                                                               | Web-standards-aligned, runs on plain Node                                                                               |
| OpenAPI / API docs     | `@hono/zod-openapi` + `@scalar/hono-api-reference`                                                       | Schema-first routes; OpenAPI 3.1 document at `/api/v1/openapi.json`, Scalar renders the reference UI at `/api/docs`     |
| Authentication         | Better Auth (`better-auth`)                                                                              | Self-hosted email/password; sessions in PostgreSQL                                                                      |
| Persistence            | PostgreSQL via raw `pg` driver                                                                           | No ORM. Numbered SQL migrations under `src/migrations/`, applied by `src/migrate.ts`.                                   |
| Scheduling             | `node-cron` in-process                                                                                   | Day-ahead price fetch jobs in `src/scheduler.ts` and `src/fetch-job.ts`                                                 |
| Rate limiting          | `hono-rate-limiter` (in-memory)                                                                          | Per-instance; no external store                                                                                         |
| Validation             | Zod                                                                                                      | Boundary validation for every external input (HTTP, env, upstream responses, persisted state)                           |
| Web UI                 | Server-rendered HTML strings (`src/ui.ts`) + a small inline client script (`src/ui-client.ts`)           | Setup-only surface per `VISION.md`. **No React, no Vite, no TanStack, no SPA.**                                         |
| Testing                | Vitest 4 (unit + integration) and Playwright (`@playwright/test`) for E2E                                | Tests live next to their subjects (`*.test.ts`); E2E under `e2e/`                                                       |
| Logging                | `console.log` / `console.warn` / `console.error` to stdout / stderr                                      | Captured by Railway's process logs. **No `pino`, no third-party logger.** PII discipline is enforced manually — see §8. |
| Build                  | `tsup`                                                                                                   | Bundles `src/` to `dist/`; `pnpm copy:migrations` copies `src/migrations/*.sql` into `dist/migrations/`                 |
| Dev runner             | `tsx`                                                                                                    | `pnpm dev` runs `tsx watch --env-file=.env src/index.ts`                                                                |
| Formatting             | Prettier 3                                                                                               |                                                                                                                         |
| Linting                | ESLint 10 with `@typescript-eslint/strict-type-checked` via the `typescript-eslint` helper (flat config) |                                                                                                                         |
| Telemetry              | none                                                                                                     | No analytics, no crash reporter, no APM by default                                                                      |

---

## 3. Build & verify commands

| Variable      | Command                                             |
| ------------- | --------------------------------------------------- |
| `$FORMAT_CMD` | `pnpm format`                                       |
| `$LINT_CMD`   | `pnpm lint`                                         |
| `$BUILD_CMD`  | `pnpm build`                                        |
| `$TEST_CMD`   | `pnpm test`                                         |
| `$VERIFY_CMD` | `pnpm test:all` (type-check → lint → tests → build) |

The `package.json` scripts are the single source of truth. Never invoke `tsc`, `eslint`, `vitest`, `playwright`, or `tsup` directly from commits, CI, or agent scripts.

---

## 4. Performance budgets

- **API request p99:** 100 ms (per route, excluding upstream calls).
- **API request p50:** 30 ms.
- **Cold start (Node):** < 2 s.
- **Memory ceiling (API container):** < 512 MB resident.

---

## 5. Persistence shape

- **Storage primitive:** **PostgreSQL only**, accessed via the raw `pg` driver. No ORM (no Drizzle, no Prisma, no Kysely).
- **Persisted entities:** declared by `VISION.md → Persistence and Privacy Posture`.
- **Schema migration policy:** numbered SQL files under `src/migrations/` (e.g. `001_baseline.sql`). Migrations are applied by `src/migrate.ts` on application startup and ship to the bundle via `pnpm copy:migrations`. Agents review every new migration before applying.
- **Forbidden persistence:** anything declared forbidden in `VISION.md → Persistence and Privacy Posture`.

---

## 6. Approved dependencies

Default answer to "should we add a library?" is **no**. New entries require a `STACK.md` PR with justification. The list below reflects the current `package.json`.

| Dependency                   | Version | Why it earns its place                                                   | Approver  | Date       |
| ---------------------------- | ------- | ------------------------------------------------------------------------ | --------- | ---------- |
| `hono`                       | `^4.12` | Backend HTTP framework — the project's chosen default                    | (default) | (template) |
| `@hono/node-server`          | `^2.0`  | Node adapter for Hono                                                    | (default) | (template) |
| `@hono/zod-openapi`          | `^1.4`  | Schema-first route definitions + OpenAPI document generation             | (default) | (template) |
| `@scalar/hono-api-reference` | `^0.10` | Renders the OpenAPI reference UI at `/api/docs`                          | (default) | (template) |
| `better-auth`                | `^1.6`  | Self-hosted email/password auth backed by the same `pg` pool             | (default) | (template) |
| `hono-rate-limiter`          | `^0.5`  | In-memory per-instance rate limiting                                     | (default) | (template) |
| `node-cron`                  | `^4.2`  | In-process cron for the day-ahead price fetch jobs                       | (default) | (template) |
| `pg`                         | `^8.20` | PostgreSQL driver                                                        | (default) | (template) |
| `zod`                        | `^4.4`  | Boundary validation for every external input                             | (default) | (template) |
| `vitest`                     | `^4.1`  | Unit + integration test runner                                           | (default) | (template) |
| `@playwright/test`           | `^1.60` | End-to-end browser tests under `e2e/`                                    | (default) | (template) |
| `eslint`                     | `^10.4` | Linter                                                                   | (default) | (template) |
| `typescript-eslint`          | `^8.59` | TS-aware lint rules + flat-config helper (`@typescript-eslint/*`)        | (default) | (template) |
| `@eslint/js`                 | `^10`   | ESLint's recommended JS rule set for the flat config                     | (default) | (template) |
| `prettier`                   | `^3.8`  | Formatter                                                                | (default) | (template) |
| `typescript`                 | `^5.9`  | Language                                                                 | (default) | (template) |
| `tsup`                       | `^8.5`  | Production bundler (`pnpm build`)                                        | (default) | (template) |
| `tsx`                        | `^4.22` | Dev runner (`pnpm dev`) and seed/migration script runner                 | (default) | (template) |
| `@better-auth/cli`           | `^1.4`  | Generates the Better Auth SQL schema consumed by the numbered migrations | (default) | (template) |
| `@types/node`                | `^25`   | Node type definitions                                                    | (default) | (template) |
| `@types/node-cron`           | `^3.0`  | Type definitions for `node-cron`                                         | (default) | (template) |
| `@types/pg`                  | `^8.20` | Type definitions for `pg`                                                | (default) | (template) |

---

## 7. Stack-specific reject-list additions

- `any` (explicit or implicit via `@typescript-eslint/no-explicit-any`) without an inline `// reason: ...` justification.
- `as` casts that bypass type checking — use `satisfies` or a runtime guard.
- `// @ts-ignore` / `// @ts-expect-error` without an inline explanation that names the underlying constraint.
- `moment` / `moment.js` — use the standard library (`Intl.DateTimeFormat`, `Temporal` via polyfill when needed) or `date-fns` only if approved.
- Full-import of `lodash` (`import _ from 'lodash'`) — import single functions only, or use the standard library equivalent.
- Raw `fetch` without zod-validated response parsing for any external network call (e.g. the Nord Pool upstream at `dataportal-api.nordpoolgroup.com`).
- Default exports for non-route, non-config modules — prefer named exports for tree-shakability and refactor safety.
- `process.env.X` reads outside a single `src/env.ts` module that validates with zod and re-exports a typed `env` constant. Test files (`*.test.ts`, `src/test-utils.ts`) are exempt where they need to set up isolated test schemas or override env per test. (`DATABASE_PUBLIC_URL` is an optional env, declared in `env.ts` but never production-required — it is read only by the offline backtest CLI's `--db` mode to reach the Railway DB from a dev machine; the server uses `DATABASE_URL`.)
- Local-time date / hour arithmetic inside calculators or storage — UTC is mandatory below the response boundary; only `formatDateTimeInTimeZone`-style conversion at the edge (`VISION.md → UTC internally`).
- Reintroducing any of the rejected stack choices: React, Vite, TanStack Router/Query, SPA build tooling, `pino` or other third-party loggers, ORMs (Drizzle/Prisma/Kysely), client-side state-management libraries (Redux/MobX/Recoil/Jotai).

**Logging exception:** direct `console.log` / `console.warn` / `console.error` calls to stdout / stderr **are** the approved logging mechanism (see §8). The reject rules above do not forbid them — they forbid the logger being replaced by a third-party library. PII still must not be logged regardless of the mechanism.

---

## 8. Logging & privacy

- **Logger:** `console.log` / `console.warn` / `console.error` writing to stdout / stderr. Railway captures both streams as the operational log sink. There is no third-party logger and no redaction middleware.
- **PII discipline (manual):** reviewers and authors must ensure no PII is logged. Specifically, the following must never appear in log output: user email addresses, password hashes, API key plaintext or hashes, session tokens, request bodies, IP addresses beyond what is needed for in-memory rate limiting. User identifiers (numeric / opaque `user_id`) may be logged only when strictly necessary to diagnose a problem; free-text user input may not.
- **Crash / error reporter:** none. If one were ever added (e.g. Sentry), it would require an entry in §6 with explicit data-flow justification.
- **Telemetry / analytics:** none.

---

## 9. Background & lifecycle

- **Allowed background work:** the `node-cron` jobs declared in `src/scheduler.ts`:
  - the day-ahead price fetch executed by `src/fetch-job.ts` — 2h baseline cadence plus a 10-minute burst window around the Nord Pool publication time;
  - the FI-forecast Fingrid grid-data fetch executed by `src/forecast-job.ts` — hourly (`0 * * * *`), fetching the public wind/consumption datasets (245/75/165/124) from `data.fingrid.fi` into the `fingrid_series` table. It fetches a ~31-day window back from now (Fingrid serves data only forward, so a wider fetch can't backfill) but retains rows for ~2 years (`RETENTION_DAYS`), pruning only beyond that — so the table accumulates grid history forward from deploy for future forecast phases, bounded to cap storage (~280k rows ≈ ~14 MB). It runs only when `FINGRID_API_KEY` is set, is wrapped in its own isolated try/catch, and its Fingrid boundary degrades rather than throwing, so a forecast failure can never affect the authoritative Nord Pool price cron or the price request path.

  These are the **only** allowed background tasks; new ones require an entry here.

- **Forbidden background work:** long-polling websockets that keep a connection alive without active user interaction; daemons that drive expensive computation on idle; service workers; any background task that retains data forbidden by `VISION.md`.

---

## 10. Intentional Divergences

| Date     | AGENTS.md rule | Divergence | Reason |
| -------- | -------------- | ---------- | ------ |
| _(none)_ | —              | —          | —      |

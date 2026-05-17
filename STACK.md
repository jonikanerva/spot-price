# STACK.md — Strict TypeScript / Node 24 LTS / Hono profile

> Single-package Node 24 + Hono + TypeScript backend for the spot-price API. Web UI is a small set of server-rendered HTML strings emitted from Hono routes (`src/ui.ts`) plus a single inline client script (`src/ui-client.ts`) — there is no React, no Vite, no SPA, no monorepo. All source lives under `src/` and is built with `tsup`; the project is deployed as a single Node process on Railway.

---

## 1. Language & Runtime

- **Primary language:** TypeScript 5.9 (latest stable; do not jump to a 6.x prerelease)
- **Strictness mode:** `"strict": true`, `"noUncheckedIndexedAccess": true`, `"exactOptionalPropertyTypes": true`, `"noImplicitOverride": true`, `"verbatimModuleSyntax": true`. ESLint with `@typescript-eslint/strict-type-checked`.
- **Target runtime:** Node.js 24 LTS (Krypton)
- **Minimum runtime version:** `>= 24.15.0` (current LTS patch). No back-deployment to Node 22 or earlier.
- **Package manager:** pnpm (single package — **no workspaces**, no `apps/`, no `packages/`)
- **Lockfile:** `pnpm-lock.yaml`

---

## 2. Frameworks

| Concern                | Framework / library                                                                            | Notes                                                                                                                   |
| ---------------------- | ---------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Backend HTTP framework | Hono + `@hono/node-server`                                                                     | Web-standards-aligned, runs on plain Node                                                                               |
| OpenAPI / API docs     | `@hono/zod-openapi` + `@scalar/hono-api-reference`                                             | Schema-first routes; Scalar renders the reference UI                                                                    |
| Authentication         | Better Auth (`better-auth`)                                                                    | Self-hosted email/password; sessions in PostgreSQL                                                                      |
| Persistence            | PostgreSQL via raw `pg` driver                                                                 | No ORM. Numbered SQL migrations under `src/migrations/`, applied by `src/migrate.ts`.                                   |
| Scheduling             | `node-cron` in-process                                                                         | Day-ahead price fetch jobs in `src/scheduler.ts` and `src/fetch-job.ts`                                                 |
| Rate limiting          | `hono-rate-limiter` (in-memory)                                                                | Per-instance; no external store                                                                                         |
| Validation             | Zod                                                                                            | Boundary validation for every external input (HTTP, env, upstream responses, persisted state)                           |
| Web UI                 | Server-rendered HTML strings (`src/ui.ts`) + a small inline client script (`src/ui-client.ts`) | Setup-only surface per `VISION.md`. **No React, no Vite, no TanStack, no SPA.**                                         |
| Testing                | Vitest 4 (unit + integration) and Playwright (`@playwright/test`) for E2E                      | Tests live next to their subjects (`*.test.ts`); E2E under `e2e/`                                                       |
| Logging                | `console.log` / `console.warn` / `console.error` to stdout / stderr                            | Captured by Railway's process logs. **No `pino`, no third-party logger.** PII discipline is enforced manually — see §8. |
| Build                  | `tsup`                                                                                         | Bundles `src/` to `dist/`; copies `src/migrations/*.sql` into `dist/migrations/`                                        |
| Dev runner             | `tsx`                                                                                          | `pnpm dev` runs `tsx watch --env-file=.env src/index.ts`                                                                |
| Formatting             | Prettier 3                                                                                     |                                                                                                                         |
| Linting                | ESLint 10 with `@typescript-eslint/strict-type-checked` (flat config)                          |                                                                                                                         |
| Telemetry              | none                                                                                           | No analytics, no crash reporter, no APM by default                                                                      |

---

## 3. Build & verify commands

| Variable      | Command                                             |
| ------------- | --------------------------------------------------- |
| `$FORMAT_CMD` | `pnpm format`                                       |
| `$LINT_CMD`   | `pnpm lint`                                         |
| `$BUILD_CMD`  | `pnpm build`                                        |
| `$TEST_CMD`   | `pnpm test`                                         |
| `$VERIFY_CMD` | `pnpm test:all` (type-check → lint → tests → build) |

The `package.json` scripts are the single source of truth. Never invoke `eslint`, `tsc`, `vitest`, `playwright`, or `tsup` directly from commits, CI, or agent scripts.

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
- **Schema migration policy:** numbered SQL files under `src/migrations/` (e.g. `001_baseline.sql`, `002_*.sql`). Migrations are applied by `src/migrate.ts` on application startup and ship to the bundle via `pnpm copy:migrations`. Agents review every new migration before applying.
- **Forbidden persistence:** anything declared forbidden in `VISION.md → Persistence and Privacy Posture`.

---

## 6. Approved dependencies

Default answer to "should we add a library?" is **no**. New entries require a `STACK.md` PR with justification. The list below reflects the current `package.json`.

| Dependency                   | Version | Why it earns its place                                                    | Approver  | Date       |
| ---------------------------- | ------- | ------------------------------------------------------------------------- | --------- | ---------- |
| `hono`                       | `^4.12` | Backend HTTP framework — the project's chosen default                     | (default) | (template) |
| `@hono/node-server`          | `^2.0`  | Node adapter for Hono                                                     | (default) | (template) |
| `@hono/zod-openapi`          | `^1.2`  | Schema-first route definitions + OpenAPI document generation              | (default) | (template) |
| `@scalar/hono-api-reference` | `^0.10` | Renders the OpenAPI reference UI at `/reference`                          | (default) | (template) |
| `better-auth`                | `^1.4`  | Self-hosted email/password auth backed by the same `pg` pool              | (default) | (template) |
| `hono-rate-limiter`          | `^0.5`  | In-memory per-instance rate limiting                                      | (default) | (template) |
| `node-cron`                  | `^4.2`  | In-process cron for the day-ahead price fetch jobs                        | (default) | (template) |
| `pg`                         | `^8.20` | PostgreSQL driver                                                         | (default) | (template) |
| `zod`                        | `^4.3`  | Boundary validation for every external input                              | (default) | (template) |
| `vitest`                     | `^4`    | Unit + integration test runner                                            | (default) | (template) |
| `@playwright/test`           | `^1.58` | End-to-end browser tests under `e2e/`                                     | (default) | (template) |
| `eslint`                     | `^10`   | Linter                                                                    | (default) | (template) |
| `@typescript-eslint/*`       | `^8`    | TS-aware lint rules (used via the `typescript-eslint` flat-config helper) | (default) | (template) |
| `prettier`                   | `^3`    | Formatter                                                                 | (default) | (template) |
| `typescript`                 | `^5.9`  | Language                                                                  | (default) | (template) |
| `tsup`                       | `^8`    | Production bundler (`pnpm build`)                                         | (default) | (template) |
| `tsx`                        | `^4`    | Dev runner (`pnpm dev`) and seed script runner                            | (default) | (template) |

---

## 7. Stack-specific reject-list additions

- `any` (explicit or implicit via `@typescript-eslint/no-explicit-any`) without an inline `// reason: ...` justification.
- `as` casts that bypass type checking — use `satisfies` or a runtime guard.
- `// @ts-ignore` / `// @ts-expect-error` without an inline explanation that names the underlying constraint.
- `moment` / `moment.js` — use the standard library (`Intl.DateTimeFormat`, `Temporal` via polyfill when needed) or `date-fns` only if approved.
- Full-import of `lodash` (`import _ from 'lodash'`) — import single functions only, or use the standard library equivalent.
- Raw `fetch` without zod-validated response parsing for any external network call (e.g. the Nord Pool upstream).
- Default exports for non-route, non-config modules — prefer named exports for tree-shakability and refactor safety.
- `process.env.X` reads outside a single `src/env.ts` module that validates with zod and re-exports a typed `env` constant. Test files (`*.test.ts`, `src/test-utils.ts`) are exempt where they need to set up isolated test schemas or override env per test.
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

- **Allowed background work:** the `node-cron` jobs declared in `src/scheduler.ts` and executed by `src/fetch-job.ts` — the day-ahead price fetch (2h baseline cadence plus a 10-minute burst window around the Nord Pool publication time). These are the **only** allowed background tasks; new ones require an entry here.
- **Forbidden background work:** long-polling websockets that keep a connection alive without active user interaction; daemons that drive expensive computation on idle; service workers; any background task that retains data forbidden by `VISION.md`.

---

## 10. Intentional Divergences

| Date     | AGENTS.md rule | Divergence | Reason |
| -------- | -------------- | ---------- | ------ |
| _(none)_ | —              | —          | —      |

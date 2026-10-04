# STACK.md — Strict TypeScript / Node 24 LTS / Hono profile

Policy revision: 2

> Single-package Node 24 + Hono + TypeScript backend for the spot-price API. The web UI is a small set of server-rendered HTML strings emitted from Hono routes (`src/ui.ts`) plus a single inline client script (`src/ui-client.ts`) — there is no React, no Vite, no SPA, no monorepo. All source lives under `src/` and is built with `tsup`; the project is deployed as a single Node process on Railway.

---

Use with `DOCTRINE.md` (P1–P9) and `CLAUDE.md`. This file maps the doctrine to concrete technology, commands, budgets, and evidence. It grants no authority beyond `CLAUDE.md`.

## 0. Project shape

- **Project risk rationale:** one household's home automations depend on this API to schedule flexible loads (sauna, EV, heating). A wrong or missing price shifts load to an expensive hour or skips it; the cost is money and comfort, not safety. Personal data is limited to one account, contract settings, and API keys (`VISION.md → Persistence and Privacy Posture`). Price and grid data are public and can be fetched again. Most failures are recoverable by a revert and redeploy; a destructive migration is the main irreversible risk.
- **Change risk:** assess each task against that rationale. Changes to price math, the cheapest-window answer, auth, API keys, migrations, or retention are material. A risk label does not waive required checks or review.
- **Layers:** interface (Hono routes, `src/ui.ts`), domain (pure transforms such as `src/calculator.ts` and `src/forecast.ts`; no framework, DB, clock, or network imports), and infrastructure (Nord Pool, Fingrid, and OpenWeatherMap clients, PostgreSQL stores), reached through narrow interfaces. Model phases as tagged unions, not parallel booleans.
- **Shape:** single-package backend service. One Hono process exposes the REST API (`/api/v1/...`), a setup-only server-rendered HTML UI (`src/ui.ts` + inline `src/ui-client.ts`), the OpenAPI 3.1 document (`/api/v1/openapi.json`) with the interactive Scalar reference (`/api/docs`), and an in-process `node-cron` price-fetch job. **No `apps/`, no `packages/`, no frontend build.**
- **Offline dev tooling:** the forecast backtest engine/CLI, `regenerate-bands`, and `backtest-metrics` live under `tools/` (not `src/`), are excluded from the production bundle (tsup's only entry is `src/index.ts`, and an ESLint guard forbids `src/` runtime from importing `tools/`), and are run on demand via `pnpm backtest` / `pnpm tsx tools/regenerate-bands.ts` — never in the server process.
- **Critical execution path:** the per-request hot path on the API (price-now / today / tomorrow / cheapest / history / forecast) and the day-ahead price fetch path (`src/fetch-job.ts`). Price math is a pure function of `(HourlyPrice, UserSettings)` in `src/calculator.ts`; the forecast estimate is a pure function of its inputs in `src/forecast.ts` (no DB, clock, or network) — the forecast route reads pre-fetched Fingrid rows off the DB and never calls Fingrid synchronously.
- **Applicable states:** API responses are typed success / typed error. When Nord Pool has not published, the answer is an explicit `available: false` / `404` — never an estimate (`VISION.md → Core Principles`). The setup UI handles awaiting-first-data, success, empty, permission/auth-blocked, and error.

---

## 1. Language & Runtime

- **Primary language:** TypeScript 5.9 (latest stable; do not jump to a 6.x prerelease)
- **Strictness mode:** `"strict": true`, `"noUncheckedIndexedAccess": true`, `"exactOptionalPropertyTypes": true`, `"noImplicitOverride": true`, `"verbatimModuleSyntax": true`, plus `"noUnusedLocals"`, `"noUnusedParameters"`, `"noFallthroughCasesInSwitch"`, `"isolatedModules"`. ESLint with `@typescript-eslint/strict-type-checked` (flat config).
- **Target runtime:** Node.js 24 LTS (Krypton)
- **Minimum runtime version:** `>= 24.15.0` (current LTS patch, pinned in `.nvmrc`). No back-deployment to Node 22 or earlier.
- **Package manager:** pnpm 10 (single package — **no workspaces**, no `apps/`, no `packages/`)
- **Lockfile:** `pnpm-lock.yaml`
- **Dev-environment provisioning:** `mise install` provisions Node, pnpm, and Python from `mise.toml`; `mise run setup` installs dependencies and Playwright browsers; `pnpm db:up` starts PostgreSQL 17 in Docker. `.nvmrc`, `package.json → engines`, and `packageManager` mirror the `mise.toml` pins.

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

| Variable      | Command                                                                                          |
| ------------- | ------------------------------------------------------------------------------------------------ |
| `$FORMAT_CMD` | `pnpm format`                                                                                    |
| `$LINT_CMD`   | `pnpm lint`                                                                                      |
| `$BUILD_CMD`  | `pnpm build`                                                                                     |
| `$TEST_CMD`   | `pnpm test`                                                                                      |
| `$VERIFY_CMD` | `pnpm test:all` (format-check → type-check → lint → production dependency audit → tests → build) |

**Required tests beyond `$VERIFY_CMD`:** `pnpm test:e2e` (Playwright). Every merge needs a passing run. Both `pnpm test:all` and `pnpm test:e2e` need the local PostgreSQL from `pnpm db:up`; `ECONNREFUSED :5432` means the database is not running, not a regression. The audit step (`pnpm audit:deps`) needs network access to the npm registry.

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
- **`fingrid_actuals` (write policy, issue #88):** public Fingrid ACTUAL series (75/124), upsert-latest keyed by `(dataset_id, start_time)`. The hourly job re-upserts its whole ~31-day window, so the upsert carries a **change guard** — `DO UPDATE … WHERE (fingrid_actuals.end_time, fingrid_actuals.value) IS DISTINCT FROM (EXCLUDED.end_time, EXCLUDED.value)`. An unchanged row is not rewritten, so it costs no new row version, no index update and no `fetched_at` bump. **`fetched_at` therefore means LAST CHANGED, not last seen** — never read it as "when did we last see this row". Public grid data, not personal. Bounded by `RETENTION_DAYS` (~730 days), pruned by `start_time`.
- **`weather_forecasts` (issue #73 Phase 1):** public OpenWeatherMap weather forecasts for a fixed set of FI points — **append-only per issuance** (PK `(point_id, issued_at, target_time)`, inserted with `ON CONFLICT DO NOTHING`, NOT the upsert-latest used by `fingrid_actuals`). One row per (point, issuance hour, target hour) preserves what the forecast said at each issue time so a later weather-feature backtest stays leakage-free; never collapse it to the latest issuance. Public weather data, not personal. Bounded by `WEATHER_RETENTION_DAYS` (~400 days) — it accumulates forward from deploy and prunes only beyond retention.
- **`weather_daily_forecasts` (issue #93):** the DAILY block of the same OpenWeatherMap response — **append-only per issuance** (PK `(point_id, issued_at, target_date)`, `ON CONFLICT DO NOTHING`, never `DO UPDATE`). Collected per day: the six `temp` sub-fields, `clouds`, `uvi`, and the solar bounds `sunrise` / `sunset` (**nullable** — One Call omits them at polar latitudes). `target_date` is the **UTC calendar date** of the raw `dt`, which is stored alongside as `target_dt` for provenance; no local-time arithmetic is used (`STACK.md §7`), and the derivation is exact for any point between UTC−11 and UTC+11. **No index beyond the PK** — the read path is designed in issue #77. Public weather data, not personal. Shares `WEATHER_RETENTION_DAYS` (~400 days) with `weather_forecasts` and gets no constant of its own; ~154k rows ≈ ~31 MB in steady state. Kill commitment: if issue #77 is closed at its checkpoint, this table is dropped in the same PR.
- **`fingrid_forecasts` (issue #78, narrowed by issue #90):** per-issuance vintages of the Fingrid FORECAST datasets (245/165) — **append-only per issuance** (PK `(dataset_id, issued_at, start_time)`, inserted with `ON CONFLICT DO NOTHING`, NOT the upsert-latest used by `fingrid_actuals`). An issuance archives **only targets with `start_time >= issued_at − VINTAGE_BACKFILL_HOURS` (6 h)**, not the whole 34-day fetch window: a past target is not a forecast, and re-archiving the window every hour wrote ~6 528 rows/hour and filled the 5 GB volume. The 6 h backfill keeps the job self-repairing across a deploy or a Fingrid outage. Both guards (dataset, target window) live INSIDE `storeFingridForecastVintages` — a caller cannot widen them. Public grid data, not personal. Bounded by `VINTAGE_RETENTION_DAYS` (180 days), pruned by `issued_at`. **Live read shape (do not change).** The latest-per-target read (`getFingridForecastVintagesLatest`) is a LATERAL skip-scan, not `DISTINCT ON`. Postgres has no loose index scan for `DISTINCT ON`. At 180-day depth the planner therefore reads every in-range issuance and sorts it to disk. That sort breaks the §4 p99 budget. The index `idx_fingrid_forecasts_target_issued` `(dataset_id, start_time, issued_at DESC)` serves both legs. The inner `SELECT DISTINCT start_time` runs as an index-only scan. The per-target LATERAL is one index seek plus `LIMIT 1`. Never simplify the live read to `DISTINCT ON`.
- **Forbidden persistence:** anything declared forbidden in `VISION.md → Persistence and Privacy Posture`.

---

## 6. Approved dependencies

Default answer to "should we add a library?" is **no**. New entries require a `STACK.md` PR with justification. The list below reflects the current `package.json`.

| Dependency                   | Version | Why it earns its place                                                               | Approver      | Date       |
| ---------------------------- | ------- | ------------------------------------------------------------------------------------ | ------------- | ---------- |
| `hono`                       | `^4.13` | Backend HTTP framework — the project's chosen default                                | (default)     | (template) |
| `@hono/node-server`          | `^2.1`  | Node adapter for Hono                                                                | (default)     | (template) |
| `@hono/zod-openapi`          | `^1.4`  | Schema-first route definitions + OpenAPI document generation                         | (default)     | (template) |
| `@scalar/hono-api-reference` | `^0.10` | Renders the OpenAPI reference UI at `/api/docs`                                      | (default)     | (template) |
| `better-auth`                | `^1.6`  | Self-hosted email/password auth backed by the same `pg` pool                         | (default)     | (template) |
| `hono-rate-limiter`          | `^0.5`  | In-memory per-instance rate limiting                                                 | (default)     | (template) |
| `node-cron`                  | `^4.2`  | In-process cron for the day-ahead price fetch jobs                                   | (default)     | (template) |
| `pg`                         | `^8.20` | PostgreSQL driver                                                                    | (default)     | (template) |
| `zod`                        | `^4.4`  | Boundary validation for every external input                                         | (default)     | (template) |
| `vitest`                     | `^4.1`  | Unit + integration test runner                                                       | (default)     | (template) |
| `@playwright/test`           | `^1.63` | End-to-end browser tests under `e2e/`                                                | (default)     | (template) |
| `eslint`                     | `^10.4` | Linter                                                                               | (default)     | (template) |
| `typescript-eslint`          | `^8.59` | TS-aware lint rules + flat-config helper (`@typescript-eslint/*`)                    | (default)     | (template) |
| `@eslint/js`                 | `^10`   | ESLint's recommended JS rule set for the flat config                                 | (default)     | (template) |
| `prettier`                   | `^3.8`  | Formatter                                                                            | (default)     | (template) |
| `typescript`                 | `^5.9`  | Language                                                                             | (default)     | (template) |
| `tsup`                       | `^8.5`  | Production bundler (`pnpm build`)                                                    | (default)     | (template) |
| `vite`                       | `^7.3`  | Peer dependency of `vitest`; pinned at a patched version so `pnpm audit` stays clean | Claude (lead) | 2026-10-04 |
| `tsx`                        | `^4.22` | Dev runner (`pnpm dev`) and seed/migration script runner                             | (default)     | (template) |
| `@better-auth/cli`           | `^1.4`  | Generates the Better Auth SQL schema consumed by the numbered migrations             | (default)     | (template) |
| `@types/node`                | `^25`   | Node type definitions                                                                | (default)     | (template) |
| `@types/node-cron`           | `^3.0`  | Type definitions for `node-cron`                                                     | (default)     | (template) |
| `@types/pg`                  | `^8.20` | Type definitions for `pg`                                                            | (default)     | (template) |

`package.json → pnpm.overrides` raises two transitive packages to patched versions: `drizzle-orm` (an optional peer of `better-auth`) and `nanoid` (from `@scalar/hono-api-reference`). Remove an override when its parent package resolves a patched version without it.

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
- Local-time date / hour arithmetic inside calculators or storage — UTC is mandatory below the response boundary; only `formatDateTimeInTimeZone`-style conversion at the edge (`VISION.md → Core Principles`, "UTC inside, local time at the edge"; see §10).
- Reintroducing any of the rejected stack choices: React, Vite, TanStack Router/Query, SPA build tooling, `pino` or other third-party loggers, ORMs (Drizzle/Prisma/Kysely), client-side state-management libraries (Redux/MobX/Recoil/Jotai).

- Fire-and-forget async work with no owner and no cancellation path. Detached work needs a why comment.
- Singletons, global mutable state, or DI containers without an entry in this file.
- Debug output, stubs, or commented-out code in shipped code.

**Logging exception:** direct `console.log` / `console.warn` / `console.error` calls to stdout / stderr **are** the approved logging mechanism (see §8). The reject rules above do not forbid them — they forbid the logger being replaced by a third-party library. PII still must not be logged regardless of the mechanism.

### Code comments

This policy narrows `DOCTRINE.md` P8 for this repository: rationale and history go to the issue or the PR, not to a comment.

A comment earns its place by stating a **constraint a reader would otherwise break** — units, ownership, failure behaviour, an actor or thread requirement, what a caller must not do. It does not describe the code. Default to none: code that needs explaining is a naming or structure defect, so fix the code first. Doc-comment an exported symbol only when the name and the signature leave a contract unstated.

The list below is the rule. The budget is a smell that points at it: a comment runs to at most 5 lines. Past that the content is usually rationale, not a constraint — move it to the issue, the PR, or `STACK.md`, and leave a pointer. A comment carrying two distinct constraints splits into two comments; it is not cut to fit. A comment over 5 lines that holds only constraints stays, and the reviewer says so. Never cut a contract to reach a number.

Never write:

- **History.** What the code used to be, what a fix changed, what a design replaced, what a measurement was. The commit, the PR, and the issue hold that record. A comment describes the present only.
- **Rationale and rejected alternatives.** Why an option lost, notes from a design session, measured numbers. These go to the issue, the PR, or `STACK.md → Scoped exceptions`.
- **A reference that does not resolve inside the repository.** Delete every issue number, PR number, and commit reference from the comment: it must still read correctly. A bare `#170` or "the previous shape" is not a reference; a named `STACK.md` section is.
- **The same explanation twice** — in a type doc and again at the call site, or in the source and again in a `STACK.md` section. Name the section instead of restating it.
- **Anything answering the current task or its author.** Tell the user instead.
- **A line number, a file offset, or a count of things elsewhere** — a later edit invalidates it silently.

Write for a reader who has the source file and nothing else: no issue, no chat, no external schema. Read each comment back cold, as a standalone sentence — an unclear referent is a defect even when the content is right. Do this while writing: a later pruning pass tests redundancy, not clarity. A note about an implementation choice sits at the line that makes it, not in the doc comment.

Keep an existing comment unless the change makes it wrong. A comment that breaks this policy is already wrong: prune it when you touch that code.

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
  - the FI-forecast Fingrid grid-data fetch executed by `src/forecast-job.ts` — hourly (`0 * * * *`), fetching the public wind/consumption datasets (245/75/165/124) from `data.fingrid.fi`. The ACTUALS (75/124) go upsert-latest into the `fingrid_actuals` table; the FORECASTS (245/165) go append-only per issuance into `fingrid_forecasts`, and since issue #90 only for targets within `VINTAGE_BACKFILL_HOURS` (6 h) before the issuance — never the whole fetch window (see §5). It fetches a ~31-day window back from now (Fingrid serves data only forward, so a wider fetch can't backfill) but retains `fingrid_actuals` rows for ~2 years (`RETENTION_DAYS`), pruning only beyond that — so the table accumulates grid history forward from deploy for future forecast phases, bounded to cap storage (2 ACTUAL datasets × 96 quarters/day × 730 days ≈ 140k rows ≈ ~7 MB). It runs only when `FINGRID_API_KEY` is set, is wrapped in its own isolated try/catch, and its Fingrid boundary degrades rather than throwing, so a forecast failure can never affect the authoritative Nord Pool price cron or the price request path.
  - the FI-forecast OpenWeatherMap weather-collection fetch executed by `src/weather-job.ts` (issue #73 Phase 1) — hourly (`0 * * * *`), fetching the One Call API 3.0 hourly forecast (48h: `temp`/`clouds`/`uvi`/`wind_speed`/`wind_deg`) for two fixed FI points (Helsinki, Vaasa) from `api.openweathermap.org` into the `weather_forecasts` table. Since issue #93 the SAME response also yields the DAILY block, stored in `weather_daily_forecasts` — **zero extra upstream calls**, because the subscription is billed per call and `exclude` only shapes the response, so the rate stays 48 calls/day. The two blocks are parsed and stored INDEPENDENTLY (two `safeParse` calls, separate transactions, per-point try/catch), so a daily schema drift can never discard the hourly rows of that issuance. Rows are **archived per issuance** (`ON CONFLICT DO NOTHING`, NOT the Fingrid upsert), so each `target_time` keeps one row per issue time — the leakage-free history a later weather-feature backtest needs. It retains ~400 days (`WEATHER_RETENTION_DAYS`), pruning only beyond that, accumulating forward from deploy. It runs only when `OPENWEATHERMAP_API_KEY` is set (so no live, billable call is made when unconfigured — e.g. in tests/E2E, where the key is never passed), is wrapped in its own isolated try/catch with per-point degrade, and its OWM boundary degrades rather than throwing, so a weather failure can never affect the authoritative Nord Pool price cron or the price request path. At ~48 calls/day it stays well inside the 1000/day free tier. Phase 1 changes no price/forecast response.

  These three are the **only** allowed background tasks; new ones require an entry here.

- **Forbidden background work:** long-polling websockets that keep a connection alive without active user interaction; daemons that drive expensive computation on idle; service workers; any background task that retains data forbidden by `VISION.md`.

---

## 10. Time & timezones

Apply `DOCTRINE.md` P5 according to meaning. `VISION.md → Core Principles` sets "UTC inside, local time at the edge".

- **Instant:** `Date`, always UTC. Store instants in PostgreSQL `timestamptz`. Exchange them as ISO-8601 with `Z`. Validate external timestamps with zod at the boundary.
- **Calendar value:** a delivery day ("today", "tomorrow") is a local calendar date in the user's IANA timezone (`user_settings.timezone`). Resolve it to a UTC range with `getUtcRangeForLocalDate` / `getUtcRangeForLocalDateSpan` in `src/time.ts`. The night-rate window (`night_start_hour`, `night_end_hour`) is a recurring local time-of-day schedule. Evaluate it per instant from the local hour; never store it as a fixed instant.
- **Duration:** a number with the unit in its name (`durationMinutes`, `RETENTION_DAYS`). A delivery day can have 23, 24, or 25 hours; never assume 24 hours per local day.
- **Conversion:** only through `Intl.DateTimeFormat` with an explicit `timeZone`, wrapped by the helpers in `src/time.ts` (for example `formatDateTimeInTimeZone`). Never compute timezone offsets by hand.
- **Tests:** inject the clock or use Vitest fake timers. Cover DST transitions for delivery-day ranges and the night window. Results must not depend on the host timezone.

---

## 11. Design guidelines & UX thresholds

The web UI is a setup surface only (`VISION.md → Core Principles`). It still must be usable without a pointer.

- **Design authority:** WCAG 2.2 AA and native HTML semantics. Use semantic elements first; use ARIA only when no native element fits.
- **Documented thresholds to exercise at the threshold:**
  - Pointer target size ≥ 24×24 CSS px (WCAG 2.5.8).
  - Text contrast ≥ 4.5:1 for body text and ≥ 3:1 for large text (WCAG 1.4.3).
  - A visible focus indicator on every interactive element (WCAG 2.4.7).
- **Input paths:** full keyboard operation; focus order follows DOM order.
- **Copy:** terse and technical, as in `VISION.md → Audience & Voice`.

---

## 12. Best practices source

Consult current, version-relevant official documentation when an API, platform rule, or material technology decision is uncertain. Use the documentation tools available in the current host. Cite the relevant section in the decision or review.

- **Sources:** Node.js (`nodejs.org/docs`), TypeScript (`typescriptlang.org/docs`), Hono (`hono.dev/docs`), Better Auth (`better-auth.com/docs`), Zod (`zod.dev`), PostgreSQL (`postgresql.org/docs`), Vitest (`vitest.dev`), Playwright (`playwright.dev`), Railway (`docs.railway.com`), MDN (`developer.mozilla.org`).
- **Upstream contracts:** Nord Pool Data Portal API, Fingrid open data (`data.fingrid.fi`), and the OpenWeatherMap One Call API 3.0 documentation.

---

## 13. Scoped exceptions

Do not weaken a rule or check to make a change pass. The responsible lead resolves exceptions within project authority, with an independent reviewer. The change author must not approve their own weaker acceptance or checks. Escalate changes beyond that authority and unresolved material risks to the owner. Record significant design decisions in concise `docs/adr/` files. Read them when relevant; keep backlog and change history in GitHub.

| Rule / scope | Reason and consequences | Compensating evidence | Responsible lead / reviewer / approval | Expiry or reassessment |
| ------------ | ----------------------- | --------------------- | -------------------------------------- | ---------------------- |
| _(none)_     | —                       | —                     | —                                      | —                      |

---

## 14. Applicability and evidence

P1–P9 refer to `DOCTRINE.md`. Run `$VERIFY_CMD` and the required tests in §3 locally for the exact version to merge, integrated with the current `main`. Failed or missing required local checks block merge. The repository has no CI; local verification and the independent review are the merge gate. Assign each check a phase: before merge or after release. Missing pre-merge evidence blocks merge. Missing post-release evidence blocks a claim of successful release. Record results for the exact commit and environment. Retain procedures, safe inputs, expected outcomes and their sources, and results for material claims. Trace expectations to original requirements, independent sources, or labelled provisional assumptions.

| Doctrine / applicability             | Required evidence                                                                                                                                           | Environment / gap to resolve                                                                                                                                                                                                                                             |
| ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| P1, P6: every task                   | Acceptance criteria, material failure cases, assumptions, and their check mapping in the issue or PR                                                        | Lead prepares; independent review for material changes                                                                                                                                                                                                                   |
| P2–P5: changed code and dependencies | `pnpm test:all`: Prettier check, strict `tsc` (`src/` and `tools/`), ESLint `strict-type-checked`, Vitest, `tsup` build; dependency rationale in §6         | Local with PostgreSQL from `pnpm db:up`                                                                                                                                                                                                                                  |
| P5–P6: changed data boundaries       | zod validation of every upstream response; tests for transformations, one invalid item among valid items, and whole-operation failure where each can occur  | Upstreams are faked in tests; no live Nord Pool, Fingrid, or OpenWeatherMap call in tests                                                                                                                                                                                |
| P3, P7: security and dependencies    | `pnpm audit:deps` (`pnpm audit --prod --audit-level high`) inside `pnpm test:all`; triage moderate and low findings in the PR                               | **Gap:** dev-only advisories are outside the gate until the `follow-up` issue that clears them closes. **Gap:** no secret scanner; compensating controls are `.gitignore` for `.env*`, the `.env` read guard, and review of every diff. The audit needs the npm registry |
| P5–P7: API and web journeys          | Vitest route and integration tests; `pnpm test:e2e` (Playwright, Chromium) for setup, dashboard, and API-key journeys; migration tests when storage changes | Local; requires Playwright browsers from `mise run setup`                                                                                                                                                                                                                |
| P4: performance                      | §4 budgets; profile hot-path changes and record the measurement in the PR                                                                                   | **Gap:** no automated budget check                                                                                                                                                                                                                                       |
| P7: release and recovery             | Before merge: migration and recovery evidence for schema changes. After release: deployed version, `/health`, and smoke results (§15)                       | Railway production; the owner merges                                                                                                                                                                                                                                     |
| P8–P9: material changes              | Current setup instructions, significant ADRs, independent `/codereview` of the integrated result, explicit limitations                                      | Separate reviewer context                                                                                                                                                                                                                                                |

Enforced boundaries: an ESLint rule forbids `src/` from importing `tools/`. Not enforced mechanically: the domain/infrastructure layer split (§0), complexity limits, and unused exports. Review covers them.

---

## 15. Release, recovery, and maintenance

- **Release:** Railway service `spot-price` (project `calmdonut`, environment `production`) builds `main` from `jonikanerva/spot-price` with Railpack and deploys every push to `main`. A merge is a release. The owner reserves merge (`CLAUDE.md → Project-specific rules`). `src/migrate.ts` applies pending migrations on start. The container memory limit is 375 MB, below the §4 ceiling.
- **Observe:** Railway gates the deploy on `GET /health` (30 s timeout). After a deploy, confirm the deployed commit in Railway, then run `BASE_URL=https://spot.calmdonut.com pnpm smoke` or the steps in `RAILWAY.md → Smoke test after deploy`. Diagnostic source: Railway process logs. Watch the next scheduled price fetch.
- **Recover:** revert the merge commit through a PR, or redeploy the previous Railway deployment. Migrations are forward-only. A migration that drops or rewrites data needs a tested recovery path before merge; Railway's managed PostgreSQL backups are the last resort.
- **Data:** the stores and retention rules are in §5 and `VISION.md → Persistence and Privacy Posture`. Price, grid, and weather data are public and can be fetched again within upstream limits. Account, settings, and API keys are the only personal data.

Main stays production-ready. Added cost, a new provider or external data transfer, material lock-in, a product change, and irreversible production-data changes outside the approved retention rules need owner approval.

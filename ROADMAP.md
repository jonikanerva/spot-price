# Roadmap

> Forward-looking plan for spot-price. The audit trail of _what happened and why_ is git history + PR descriptions + merge-commit chains on `main`. This file plans what is coming next.

## Status

The product is **in production** at the user's self-hosted Railway instance. There is currently **one milestone in progress**: the authenticated price-history endpoint. New work is added here only when a concrete milestone is scoped.

## Milestones

| #   | Status      | Milestone                            | Scope summary                                                                                                                                       | PR                                                       |
| --- | ----------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| 1   | In progress | Authenticated price-history endpoint | Read-only GET /api/v1/price/history?from=&to= returning total prices for a past local-date range; JSON-only, 31-day cap, current-settings semantics | [#53](https://github.com/jonikanerva/spot-price/pull/53) |

Statuses: `Todo` · `In progress` · `Done` · `Blocked` · `Needs human`.

## Strategic decisions in force

_Active architectural and product constraints that bind future work. When a decision is superseded, rewrite or remove the entry; the git history of this file preserves the prior state._

- **Single-package monolith.** No monorepo, no `apps/` or `packages/`. All source under `src/`. Why it binds future work: keeps the project surface small and operationally simple for a single-tenant self-hosted instance per `VISION.md`.
- **Server-rendered HTML UI, not a SPA.** The web UI is HTML strings emitted by Hono routes (`src/ui.ts`) plus a small inline client script (`src/ui-client.ts`). No React, no Vite, no client-side framework. Why it binds future work: `VISION.md` defines the UI as a setup surface only ("the API is the product, the UI is for setup"); a SPA stack would add complexity without product value.
- **Raw `pg` driver + numbered SQL migrations.** No ORM (no Drizzle, no Prisma). Migrations live as `.sql` files under `src/migrations/` and are applied by `src/migrate.ts`. Why it binds future work: PR #48 completed the SQLite-to-PostgreSQL migration with this shape; introducing an ORM now would re-do that work for no measured benefit.
- **`console.*` logging to stdout/stderr.** No `pino`, no structured-logging middleware. Railway captures stdout/stderr as the operational log sink. Why it binds future work: keeps the dependency surface minimal; PII discipline is enforced manually per `STACK.md §8`.
- **`price/history` endpoint is JSON-only, ≤31-day span, no UI browser, no range aggregation.** Totals apply the user's CURRENT contract settings to historical public spot data; no historical settings versioning. Why it binds future work: `VISION.md` Guardrails reject historical price browsers and a general home-energy dashboard — re-run the `VISION.md` decision filter before any history feature crosses these four lines.

## Open risks

_Risks currently threatening planned work. When mitigated or no longer relevant, delete the row._

_None currently tracked._

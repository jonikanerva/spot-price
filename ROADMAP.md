# Roadmap

> Forward-looking plan for spot-price. The audit trail of _what happened and why_ is git history + PR descriptions + merge-commit chains on `main`. This file plans what is coming next.

## Status

The product is **in production** at the user's self-hosted Railway instance. There are currently **no planned next milestones**. New work is added here only when a concrete milestone is scoped.

## Milestones

| #   | Status | Milestone | Scope summary | PR  |
| --- | ------ | --------- | ------------- | --- |

_No milestones currently planned._

Statuses: `Todo` · `In progress` · `Done` · `Blocked` · `Needs human`.

## Strategic decisions in force

_Active architectural and product constraints that bind future work. When a decision is superseded, rewrite or remove the entry; the git history of this file preserves the prior state._

- **Single-package monolith.** No monorepo, no `apps/` or `packages/`. All source under `src/`. Why it binds future work: keeps the project surface small and operationally simple for a single-tenant self-hosted instance per `VISION.md`.
- **Server-rendered HTML UI, not a SPA.** The web UI is HTML strings emitted by Hono routes (`src/ui.ts`) plus a small inline client script (`src/ui-client.ts`). No React, no Vite, no client-side framework. Why it binds future work: `VISION.md` defines the UI as a setup surface only ("the API is the product, the UI is for setup"); a SPA stack would add complexity without product value.
- **Raw `pg` driver + numbered SQL migrations.** No ORM (no Drizzle, no Prisma). Migrations live as `.sql` files under `src/migrations/` and are applied by `src/migrate.ts`. Why it binds future work: PR #48 completed the SQLite-to-PostgreSQL migration with this shape; introducing an ORM now would re-do that work for no measured benefit.
- **`console.*` logging to stdout/stderr.** No `pino`, no structured-logging middleware. Railway captures stdout/stderr as the operational log sink. Why it binds future work: keeps the dependency surface minimal; PII discipline is enforced manually per `STACK.md §8`.

## Open risks

_Risks currently threatening planned work. When mitigated or no longer relevant, delete the row._

_None currently tracked._

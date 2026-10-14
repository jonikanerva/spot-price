# 0001. The Rust port ships in one release with a data-compatible revert path

- Status: accepted
- Date: 2026-10-14

## Context

The owner asked for the whole Rust port in one PR. The port must be easy to
test locally and in production, and easy to revert. Home automations depend
on the API every day, so a failed release must be cheap to undo.

The production database has the schema of the TypeScript migrations
`001`–`007` and their ledger table `_migrations`. Railway builds `main` with
Railpack and deploys each merge. The default Rust profile (STACK-RUST §5)
runs migrations as a separate release step, not as a side effect of the
server start.

## Decision

1. The port replaces the TypeScript service in one PR. The schema, the
   environment variable names, the endpoints, and the `/api/v1` responses
   stay the same.
2. `migrations/0001_baseline.sql` is idempotent. On the production schema it
   changes nothing. On an empty database it creates the same schema as the
   TypeScript migrations. The `_migrations` table stays in place.
3. New tables are additive only. The port adds `app_sessions` and the SQLx
   ledger `_sqlx_migrations`. The port does not drop, rename, or rewrite any
   existing table, column, or row.
4. The start command in `railpack.json` runs `spot-price migrate` and then
   `exec spot-price serve`. `serve` refuses to start on an unmigrated
   database. The start command is versioned with the code.
5. The Rust service writes user and account rows in the Better Auth format
   (ADR 0002), so the TypeScript service accepts users that the Rust service
   created.

## Alternatives

- **Railway pre-deploy command.** Rejected: it is a service setting outside
  the repository. A code revert would leave it in place, and the TypeScript
  image has no `spot-price` binary, so the revert deploy would fail.
- **Migrate inside `serve`.** Rejected: an implicit start-up effect, against
  the profile, with no benefit over the start command.
- **A staged port over several PRs.** Rejected by the owner: two services
  side by side need routing and a longer period with two code bases.

## Consequences

- `git revert -m 1` of the merge removes `railpack.json` and the Rust
  sources. Railpack then detects the Node service again. A Railway rollback
  restores the old image at once.
- After a revert, `_sqlx_migrations` and `app_sessions` stay. The TypeScript
  service does not read them. A session created by the Rust service does
  not work after a revert, because the TypeScript service does not read
  `app_sessions`. A Better Auth session that has not expired still works.
- The TypeScript service cannot start on a database that the Rust service
  created from empty: its migration `006` fails. Production was created by
  the TypeScript service, so this does not affect the revert path.
- Evidence (2026-10-14, local): on a database created by the TypeScript
  service, the Rust `migrate` added only `_sqlx_migrations` and
  `app_sessions`. A user created on each service signed in on the other,
  with the same password and the same API key. The TypeScript migrator then
  applied nothing.
- During a deploy, the old and the new service run their jobs for a short
  time. Both write the same values. The primary keys, `ON CONFLICT`, and the
  change guard make this safe.

## Cost of reversal

Low while the revert path holds: a Railway rollback or a code revert.

## Reassessment

When a product change needs a schema change that the TypeScript service
cannot run on. Then the owner decides to end the revert path, and a new ADR
supersedes this one.

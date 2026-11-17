# Railway Deployment Guide

`STACK.md → Release, recovery, and maintenance` is the source of truth. This
page is a short checklist.

## Service setup

1. Create a Railway project from this repository.
2. Add one service for the app. Railpack builds it from `railpack.json`
   (Rust provider). Do not pin the Railpack version; Railway selects it.
3. The database is the Neon project `spot-price` (`STACK.md` §5). Set
   `DATABASE_URL` to the direct Neon URL with `sslmode=verify-full`. Do not
   use the `-pooler` endpoint.

## Required environment variables

- `NODE_ENV=production`
- `DATABASE_URL` (the direct Neon URL; set it with `railway variables --set`
  from the shell, so the value never appears in a log or a chat)
- `BETTER_AUTH_URL=https://spot.calmdonut.com` (the public origin)
- `BETTER_AUTH_SECRET=<32+ char random secret>` (unused by the service; its removal is a follow-up)
- Optional: `FINGRID_API_KEY`, `OPENWEATHERMAP_API_KEY`, `LOG_FORMAT`

Railway sets `PORT`.

## Before a release

Record the active deployment and its commit in the PR.

Before the merge of a release that adds a migration, or that changes the
database connection, `BETTER_AUTH_URL`, or the start command, run these
checks read-only against production (`STACK.md → Release procedure`):

The Mac has no `psql`; the conda client of mise is the client. `PGOPTIONS`
makes the session read-only:

```bash
NEON="$(neon connection-string --project-id autumn-dream-98820540 \
  --database-name neondb --role-name neondb_owner | sed 's/?.*$//')?sslmode=verify-full&sslrootcert=system"
pgc() { PGOPTIONS='-c default_transaction_read_only=on' \
  mise x conda:postgresql@18.7 -- psql "$@"; }
pgc "$NEON" -X -q -A -t -f - <scripts/schema-snapshot.sql \
  | diff crates/server/tests/fixtures/schema-snapshot.txt -
pgc "$NEON" -X -q -A -t -f - <scripts/release-precheck.sql
```

- The snapshot differs only by the Rust migrations that production already
  has, and by the objects of the extension `pg_stat_statements`. Through
  migration 4, the migrations add `app_sessions` and `contract_terms` and
  drop three indexes (`STACK.md` §5).
- The precheck prints `typescript_migrations|1,2,3,4,5,6,7`, `0` for each
  `user_settings_invalid_*` count, and two `contract_terms` lines
  (`STACK.md` §15).
- `DATABASE_URL` uses the direct Neon endpoint (no `-pooler` in the host
  name) with `sslmode=verify-full`.
- `BETTER_AUTH_URL` is exactly `https://spot.calmdonut.com`, with no trailing
  slash.

## Recovery

- Fix a bad release with a revert PR or a fix PR. Do not use the Railway
  rollback after a migration (owner decision, 2026-10-19).
- The Neon history window of 6 hours is the only restore path for the
  database. Older data cannot be restored; the owner accepts this risk
  (`STACK.md` §5).
- No rollback database exists. The owner removed the Railway service
  `Postgres spot-price` and its volume on 2026-11-17. A move away from Neon
  is a data move to a new database (`STACK.md` §5 and §15).

## Start and health check

- Start command (in `railpack.json`):
  `./bin/spot-price migrate && exec ./bin/spot-price serve`
- Health check path: `/health`
- Expected: `200` with `{"status":"ok","db":"connected"}`

## Smoke test

`mise run smoke` creates an account and an API key. Run it only against a
local instance (`BASE_URL`, default `http://localhost:3000`), or on a first
deploy with the owner present. Never run it against production.

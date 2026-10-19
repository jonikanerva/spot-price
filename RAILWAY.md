# Railway Deployment Guide

`STACK.md → Release, recovery, and maintenance` is the source of truth. This
page is a short checklist.

## Service setup

1. Create a Railway project from this repository.
2. Add one service for the app. Railpack builds it from `railpack.json`
   (Rust provider). Do not pin the Railpack version; Railway selects it.
3. Add a PostgreSQL service. Railway sets `DATABASE_URL`.

## Required environment variables

- `NODE_ENV=production`
- `DATABASE_URL` (set by Railway)
- `BETTER_AUTH_URL=https://spot.calmdonut.com` (the public origin)
- `BETTER_AUTH_SECRET=<32+ char random secret>` (kept for the revert path)
- Optional: `FINGRID_API_KEY`, `OPENWEATHERMAP_API_KEY`, `LOG_FORMAT`

Railway sets `PORT`.

## Before a release

Record the active deployment and its commit in the PR.

Before the merge of a release that adds a migration, or that changes the
database connection, `BETTER_AUTH_URL`, or the start command, run these
checks read-only against production (`STACK.md → Release procedure`):

```bash
PGOPTIONS='-c default_transaction_read_only=on' \
  psql "$DATABASE_PUBLIC_URL" -X -q -A -t -f scripts/schema-snapshot.sql \
  | diff crates/server/tests/fixtures/schema-snapshot.txt -
PGOPTIONS='-c default_transaction_read_only=on' \
  psql "$DATABASE_PUBLIC_URL" -X -q -A -t -f scripts/release-precheck.sql
```

- The snapshot differs only by the Rust migrations that production already
  has. Through migration 3, that is the three indexes that migration 3 drops
  (`STACK.md` §5).
- The precheck prints `typescript_migrations|1,2,3,4,5,6,7` and `0` for each
  `user_settings_invalid_*` count.
- `DATABASE_URL` has no `sslmode`, or `sslmode=prefer` or `sslmode=disable`.
  The service has no `PGSSLMODE` variable, or it is `prefer` or `disable`.
- The database certificate is version 3 (`STACK.md` §15 has the `openssl`
  command). The Rust service rejects a version 1 certificate.
- `BETTER_AUTH_URL` is exactly `https://spot.calmdonut.com`, with no trailing
  slash.

## Recovery

- Fix a bad release with a revert PR or a fix PR. Do not use the Railway
  rollback after a migration (owner decision, 2026-10-19).
- The Railway Hobby plan has no database backups. A lost database cannot be
  restored; the owner accepts this risk (`STACK.md` §5).

## Start and health check

- Start command (in `railpack.json`):
  `./bin/spot-price migrate && exec ./bin/spot-price serve`
- Health check path: `/health`
- Expected: `200` with `{"status":"ok","db":"connected"}`

## Smoke test

`mise run smoke` creates an account and an API key. Run it only against a
local instance (`BASE_URL`, default `http://localhost:3000`), or on a first
deploy with the owner present. Never run it against production.

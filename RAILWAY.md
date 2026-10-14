# Railway Deployment Guide

`STACK.md → Release, recovery, and maintenance` is the source of truth. This
page is a short checklist.

## Service setup

1. Create a Railway project from this repository.
2. Add one service for the app. Railpack builds it from `railpack.json`
   (Rust provider).
3. Add a PostgreSQL service. Railway sets `DATABASE_URL`.

## Required environment variables

- `NODE_ENV=production`
- `DATABASE_URL` (set by Railway)
- `BETTER_AUTH_URL=https://spot.calmdonut.com` (the public origin)
- `BETTER_AUTH_SECRET=<32+ char random secret>` (kept for the revert path)
- Optional: `FINGRID_API_KEY`, `OPENWEATHERMAP_API_KEY`, `LOG_FORMAT`

Railway sets `PORT`.

## Start and health check

- Start command (in `railpack.json`):
  `./bin/spot-price migrate && exec ./bin/spot-price serve`
- Health check path: `/health`
- Expected: `200` with `{"status":"ok","db":"connected"}`

## Smoke test

`mise run smoke` creates an account and an API key. Run it only against a
local instance (`BASE_URL`, default `http://localhost:3000`), or on a first
deploy with the owner present. Never run it against production.

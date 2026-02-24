# Railway Deployment Guide

## Service setup

1. Create a Railway project from this repository.
2. Add one service for the Node app.
3. Attach a volume mounted at `/app/data`.

## Required environment variables

- `NODE_ENV=production`
- `PORT=3000`
- `DATABASE_PATH=/app/data/spot-price.db`

Optional:

- `CRON_SCHEDULE` (defaults to `0 12 * * *` in code path)

## Health checks

- Path: `/health`
- Expected: `200` with `{ "status": "ok", "db": "connected" }`

## Backup checklist

- Enable daily volume backups in Railway dashboard.
- Keep at least daily + weekly retention.

## Smoke test after deploy

Run after first successful deployment:

```bash
curl -sS "$APP_URL/health"
curl -sS -X POST "$APP_URL/api/keys" -H "content-type: application/json" -d '{"userId":"smoke-user","name":"smoke"}'
```

Then call one protected endpoint with returned API key:

```bash
curl -sS "$APP_URL/api/v1/price/now" -H "Authorization: Bearer <api-key>"
```

# Railway Deployment Guide

## Service setup

1. Create a Railway project from this repository.
2. Add one service for the Node app.
3. Attach a volume mounted at `/app/data`.

## Required environment variables

- `NODE_ENV=production`
- `PORT=3000`
- `DATABASE_PATH=/app/data/spot-price.db`
- `BETTER_AUTH_SECRET=<32+ char random secret>`
- `BETTER_AUTH_URL=https://spot.calmdonut.com`

## Health checks

- Path: `/health`
- Expected: `200` with `{ "status": "ok", "db": "connected" }`

## Backup checklist (optional)

For this hobby project, backups are optional because runtime data is non-critical and can be recreated.

## Smoke test after deploy

Run after first successful deployment:

```bash
curl -sS "$APP_URL/health"
AUTH_HEADERS="$(mktemp)"
AUTH_BODY="$(mktemp)"

curl -sS -D "$AUTH_HEADERS" -o "$AUTH_BODY" -X POST "$APP_URL/api/session/login-or-signup" \
  -H "content-type: application/json" \
  -d '{"username":"smoke_user","password":"smoke-password-123"}'

SESSION_COOKIE="$(python3 - "$AUTH_HEADERS" <<'PY'
import pathlib
import re
import sys

headers = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
match = re.search(r"^set-cookie:\\s*([^;\\r\\n]+)", headers, flags=re.IGNORECASE | re.MULTILINE)
print(match.group(1) if match else "")
PY
)"

curl -sS "$APP_URL/api/keys" -H "Cookie: $SESSION_COOKIE"
```

Then call one protected endpoint with returned API key:

```bash
curl -sS "$APP_URL/api/v1/price/now" -H "Authorization: Bearer <api-key>"
```

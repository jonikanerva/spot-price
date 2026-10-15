#!/usr/bin/env bash
# Smoke check of a locally running Rust service (`spot-price serve`).
# It creates an account and an API key, so never run it against production.
#
# Usage: BASE_URL=http://localhost:3000 scripts/smoke-local.sh
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:3000}"
USERNAME="smoke_$(date +%s)"
PASSWORD="smoke-password-123"

HEADERS_FILE="$(mktemp)"
BODY_FILE="$(mktemp)"
trap 'rm -f "$HEADERS_FILE" "$BODY_FILE"' EXIT

echo "[smoke] Health check"
curl -fsS "$BASE_URL/health" >/dev/null

echo "[smoke] Login or signup"
STATUS_CODE=$(curl -sS -D "$HEADERS_FILE" -o "$BODY_FILE" -w '%{http_code}' \
  -X POST "$BASE_URL/api/session/login-or-signup" \
  -H "content-type: application/json" \
  -d "{\"username\":\"$USERNAME\",\"password\":\"$PASSWORD\"}")
if [[ "$STATUS_CODE" != "200" ]]; then
  echo "[smoke] Failed: auth status $STATUS_CODE"
  cat "$BODY_FILE"
  exit 1
fi

# The first `name=value` pair of the Set-Cookie header.
SESSION_COOKIE=$(python3 -c '
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
match = re.search(r"^set-cookie:\s*([^;\r\n]+)", text, flags=re.IGNORECASE | re.MULTILINE)
print(match.group(1) if match else "")
' "$HEADERS_FILE")
if [[ -z "$SESSION_COOKIE" ]]; then
  echo "[smoke] Failed: session cookie missing"
  exit 1
fi

echo "[smoke] Get API key via session"
KEY_JSON=$(curl -fsS "$BASE_URL/api/keys" -H "Cookie: $SESSION_COOKIE")
API_KEY=$(python3 -c '
import json, sys
value = json.loads(sys.argv[1]).get("apiKey", "")
print(value if isinstance(value, str) else "")
' "$KEY_JSON")
if [[ -z "$API_KEY" ]]; then
  # The body can hold a key, so it is not printed.
  echo "[smoke] Failed: apiKey missing"
  exit 1
fi

echo "[smoke] Price now"
curl -fsS "$BASE_URL/api/v1/price/now" -H "Authorization: Bearer $API_KEY" >/dev/null

echo "[smoke] Cheapest window"
curl -fsS "$BASE_URL/api/v1/price/cheapest?duration=180" -H "Authorization: Bearer $API_KEY" >/dev/null

echo "[smoke] OK"

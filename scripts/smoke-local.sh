#!/usr/bin/env bash
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
curl -sS -D "$HEADERS_FILE" -o "$BODY_FILE" -X POST "$BASE_URL/api/session/login-or-signup" \
  -H "content-type: application/json" \
  -d "{\"username\":\"$USERNAME\",\"password\":\"$PASSWORD\"}"

STATUS_CODE=$(awk 'NR==1 { print $2 }' "$HEADERS_FILE")
if [[ "$STATUS_CODE" != "200" ]]; then
  echo "[smoke] Failed: auth status $STATUS_CODE"
  cat "$BODY_FILE"
  exit 1
fi

SESSION_COOKIE=$(python3 - "$HEADERS_FILE" <<'PY'
import pathlib
import re
import sys

headers_path = pathlib.Path(sys.argv[1])
content = headers_path.read_text(encoding="utf-8")
match = re.search(r"^set-cookie:\s*([^;\r\n]+)", content, flags=re.IGNORECASE | re.MULTILINE)
print(match.group(1) if match else "")
PY
)

if [[ -z "$SESSION_COOKIE" ]]; then
  echo "[smoke] Failed: session cookie missing"
  cat "$BODY_FILE"
  exit 1
fi

echo "[smoke] Get API key via session"
KEY_JSON=$(curl -fsS "$BASE_URL/api/keys" \
  -H "Cookie: $SESSION_COOKIE")

API_KEY=$(python3 - <<'PY'
import json,sys
obj=json.loads(sys.stdin.read())
print(obj.get("apiKey",""))
PY
<<< "$KEY_JSON")

if [[ -z "$API_KEY" ]]; then
  echo "[smoke] Failed: apiKey missing"
  echo "$KEY_JSON"
  exit 1
fi

echo "[smoke] Price now"
curl -fsS "$BASE_URL/api/v1/price/now" -H "Authorization: Bearer $API_KEY" >/dev/null

echo "[smoke] Cheapest window"
curl -fsS "$BASE_URL/api/v1/price/cheapest?duration=180" -H "Authorization: Bearer $API_KEY" >/dev/null

echo "[smoke] OK"

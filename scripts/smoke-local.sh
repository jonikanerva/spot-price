#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:3000}"

echo "[smoke] Health check"
curl -fsS "$BASE_URL/health" >/dev/null

echo "[smoke] Create API key"
KEY_JSON=$(curl -fsS -X POST "$BASE_URL/api/keys" \
  -H "content-type: application/json" \
  -d '{"userId":"smoke-user","name":"smoke"}')

API_KEY=$(python3 - <<'PY'
import json,sys
obj=json.loads(sys.stdin.read())
print(obj.get("apiKey",""))
PY
<<< "$KEY_JSON")

if [[ -z "$API_KEY" ]]; then
  echo "[smoke] Failed: apiKey missing"
  exit 1
fi

echo "[smoke] Price now"
curl -fsS "$BASE_URL/api/v1/price/now" -H "Authorization: Bearer $API_KEY" >/dev/null

echo "[smoke] Cheapest window"
curl -fsS "$BASE_URL/api/v1/price/cheapest?duration=180" -H "Authorization: Bearer $API_KEY" >/dev/null

echo "[smoke] OK"

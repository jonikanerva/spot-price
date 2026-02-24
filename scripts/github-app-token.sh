#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${GH_APP_ID:-}" ]]; then
  echo "GH_APP_ID is required" >&2
  exit 1
fi

if [[ -z "${GH_APP_INSTALLATION_ID:-}" ]]; then
  echo "GH_APP_INSTALLATION_ID is required" >&2
  exit 1
fi

if [[ -z "${GH_APP_PRIVATE_KEY_PATH:-}" ]]; then
  echo "GH_APP_PRIVATE_KEY_PATH is required" >&2
  exit 1
fi

if [[ ! -f "${GH_APP_PRIVATE_KEY_PATH}" ]]; then
  echo "Private key file not found: ${GH_APP_PRIVATE_KEY_PATH}" >&2
  exit 1
fi

b64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

now="$(date +%s)"
iat="$((now - 60))"
exp="$((now + 540))"

header='{"alg":"RS256","typ":"JWT"}'
payload="{\"iat\":${iat},\"exp\":${exp},\"iss\":${GH_APP_ID}}"

header_b64="$(printf '%s' "${header}" | b64url)"
payload_b64="$(printf '%s' "${payload}" | b64url)"
unsigned="${header_b64}.${payload_b64}"

signature_b64="$(printf '%s' "${unsigned}" | openssl dgst -binary -sha256 -sign "${GH_APP_PRIVATE_KEY_PATH}" | b64url)"
jwt="${unsigned}.${signature_b64}"

response="$(curl -sS -X POST \
  -H "Authorization: Bearer ${jwt}" \
  -H "Accept: application/vnd.github+json" \
  "https://api.github.com/app/installations/${GH_APP_INSTALLATION_ID}/access_tokens")"

token="$(python3 -c 'import json,sys; data=json.load(sys.stdin); print(data.get("token",""))' <<<"${response}")"

if [[ -z "${token}" ]]; then
  echo "Failed to mint GitHub App installation token" >&2
  echo "Response: ${response}" >&2
  exit 1
fi

printf '%s\n' "${token}"

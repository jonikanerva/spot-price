#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"

read_setting() {
  local env_name="$1"
  local git_key="$2"
  local fallback=""

  if [[ -n "${!env_name:-}" ]]; then
    fallback="${!env_name}"
  else
    fallback="$(git config --local --get "${git_key}" || true)"
  fi

  printf '%s' "${fallback}"
}

require_env() {
  local name="$1"
  local value="${!name:-}"
  if [[ -z "${value}" ]]; then
    echo "${name} is required" >&2
    exit 1
  fi
  if [[ "${value}" == "<"*">" ]]; then
    echo "${name} still has placeholder value (${value}). Replace it with a real value." >&2
    exit 1
  fi
}

GH_APP_ID="$(read_setting GH_APP_ID opencode.githubAppId)"
GH_APP_INSTALLATION_ID="$(read_setting GH_APP_INSTALLATION_ID opencode.githubAppInstallationId)"
GH_APP_PRIVATE_KEY_PATH="$(read_setting GH_APP_PRIVATE_KEY_PATH opencode.githubAppPrivateKeyPath)"

export GH_APP_ID
export GH_APP_INSTALLATION_ID
export GH_APP_PRIVATE_KEY_PATH

require_env GH_APP_ID
require_env GH_APP_INSTALLATION_ID
require_env GH_APP_PRIVATE_KEY_PATH

if [[ ! -f "${GH_APP_PRIVATE_KEY_PATH}" ]]; then
  echo "Private key file not found: ${GH_APP_PRIVATE_KEY_PATH}" >&2
  exit 1
fi

if [[ -z "${GH_APP_TOKEN:-}" ]]; then
  GH_APP_TOKEN="$("${repo_root}/scripts/github-app-token.sh")"
fi

git config --local user.name "donut"
git config --local user.email "donut-bot@users.noreply.github.com"

origin_url="$(git remote get-url origin)"
if [[ "${origin_url}" == git@github.com:* ]]; then
  repo_path="${origin_url#git@github.com:}"
  git remote set-url origin "https://x-access-token@github.com/${repo_path}"
elif [[ "${origin_url}" == ssh://git@github.com/* ]]; then
  repo_path="${origin_url#ssh://git@github.com/}"
  git remote set-url origin "https://x-access-token@github.com/${repo_path}"
elif [[ "${origin_url}" == https://github.com/* ]]; then
  repo_path="${origin_url#https://github.com/}"
  git remote set-url origin "https://x-access-token@github.com/${repo_path}"
fi

git config --local --replace-all url."https://github.com/".insteadOf git@github.com:
git config --local --add url."https://github.com/".insteadOf ssh://git@github.com/

git config --local credential.https://github.com.helper "!f() { test \"\${1-}\" = get || exit 0; echo username=x-access-token; echo password=\"${GH_APP_TOKEN}\"; }; f"

echo "GitHub App auth configured for this repo."
echo "Author: $(git config --local user.name) <$(git config --local user.email)>"
echo "Origin (stored): $(git config --local --get remote.origin.url)"
echo "Git credentials for github.com are now backed by GitHub App installation token."

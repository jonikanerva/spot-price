# shellcheck shell=bash
# The spot-price settings of scripts/db.sh (STACK.md §3). scripts/db.sh is
# the same file in caltodo, mastofeed, and spot-price; the differences of
# this project are here.

DB_PROJECT=spot-price
DB_ENV=SPOT_PRICE
DB_DEV_DATABASE=spot_price
DB_TEST_DATABASE=spot_price_test
# Host ports 40000-48999. caltodo uses 20000-29999 and mastofeed
# 30000-39999. All stay below the macOS ephemeral range (49152-65535).
DB_PORT_MIN=40000
DB_PORT_SPAN=9000
DB_TLS=0
DB_PROJECT_COMMANDS="e2e"

E2E_DATABASE=spot_price_e2e
# A service port of .env.local or the shell, before scripts/db.sh runs.
SERVICE_PORT_SET="${PORT:-}"

# The service port of `mise run dev` of this worktree: kept in a state file,
# so the address stays the same between runs. Another worktree gets another
# port.
dev_app_port() {
  local port
  if [ -s "$STATE/app-port" ]; then
    APP_PORT="$(cat "$STATE/app-port")"
    return 0
  fi
  for _ in $(seq 1 50); do
    port=$((3200 + RANDOM % 700))
    if ! nc -z 127.0.0.1 "$port" 2>/dev/null; then
      echo "$port" >"$STATE/app-port"
      APP_PORT="$port"
      return 0
    fi
  done
  die "no free service port in 3200-3899"
}

# The service address of `dev`: a PORT and a BETTER_AUTH_URL of .env.local or
# the shell win. The Origin check compares BETTER_AUTH_URL byte for byte with
# the origin in the browser, so set both or neither.
project_dev_env() {
  APP_PORT=""
  if [ -n "$SERVICE_PORT_SET" ]; then
    APP_PORT="$SERVICE_PORT_SET"
  else
    dev_app_port
  fi
  export PORT="$APP_PORT" BETTER_AUTH_URL="${BETTER_AUTH_URL:-http://localhost:$APP_PORT}"
  say "service: $BETTER_AUTH_URL"
}

project_info() {
  if [ -s "$STATE/app-port" ]; then
    echo "service:     http://localhost:$(cat "$STATE/app-port") (mise run dev)"
  fi
}

# The Playwright journeys (playwright.config.ts). Runs inside with-test-db:
# the e2e database is on the test server of this run. The run has its own
# copy of the release binary, service port, and output directory, so two runs
# in one worktree do not share files.
project_command() { # $1 = e2e
  local container_name="${SPOT_PRICE_DB_CONTAINER:-}" run_dir url status port
  [ "$1" = e2e ] || return 2
  [ -n "$container_name" ] && [ -n "${DATABASE_URL:-}" ] || die "run e2e through with-test-db"
  run_dir="$ROOT/.tmp/e2e/$container_name"
  mkdir -p "$run_dir" "$STATE"
  # The release build and the copy under one lock: another run of this
  # worktree can rebuild the binary at the same time.
  lock "$STATE/release-build.lock" "the release build lock of this worktree"
  SQLX_OFFLINE=true cargo build --release --locked --package spot-price-server
  cp "$ROOT/target/release/spot-price" "$run_dir/spot-price"
  unlock "$STATE/release-build.lock"
  psql_in "$container_name" postgres \
    --command="DROP DATABASE IF EXISTS $E2E_DATABASE WITH (FORCE)" \
    --command="CREATE DATABASE $E2E_DATABASE"
  url="${DATABASE_URL%/*}/$E2E_DATABASE"
  DATABASE_URL="$url" LOG_FORMAT=text "$run_dir/spot-price" migrate
  container exec --interactive "$container_name" psql --quiet --no-psqlrc --username=postgres \
    --dbname="$E2E_DATABASE" -v ON_ERROR_STOP=1 <"$ROOT/e2e/seed.sql"
  # A port that was free a moment ago. Playwright fails on a port in use; it
  # never talks to a server of another run.
  port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  status=0
  E2E_DATABASE_URL="$url" E2E_SERVER="$run_dir/spot-price" E2E_PORT="$port" \
    E2E_OUTPUT_DIR="$run_dir/test-results" pnpm exec playwright test || status=$?
  if [ $status -eq 0 ]; then
    rm -r -f "${run_dir:?}"
  else
    say "e2e failed; traces and results stay in ${run_dir#"$ROOT"/}"
  fi
  return $status
}

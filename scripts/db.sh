#!/usr/bin/env bash
# Local PostgreSQL in Apple containers (STACK.md §3).
#
# The Mac has no PostgreSQL install. Each database server is a container of
# the image in SPOT_PRICE_PG_IMAGE (mise.toml). There are two kinds:
#
# - Dev server: one per worktree. It keeps its data in a named volume, uses
#   the durable PostgreSQL defaults, and stays until `db:stop`. `dev` uses it.
# - Test server: one per independent run of `test`, `e2e`, `sqlx:check`,
#   `sqlx:prepare`, or `verify`. Its data is in memory (tmpfs) with fsync off.
#   The run removes it at the end.
#
# The worktree id is a hash of the physical checkout path, so two worktrees
# never share a container, even on the same branch. A test server name adds
# a random run id. Ports are published on 127.0.0.1 only. Every connection
# needs the password of its server.
#
# Usage: scripts/db.sh <command>
#   setup                       check the container tool, fetch the image
#   start | stop | info | psql  the dev server of this worktree
#   with-dev-db -- CMD...       run CMD with DATABASE_URL of the dev server
#   with-test-db [--share] -- CMD...
#                               run CMD with DATABASE_URL of a new test server
#   dump FILE                   write the dev database to FILE (pg_dump -Fc)
#   restore FILE                load FILE into the empty dev database
#   clone WORKTREE              copy the dev database of another worktree
#                               into the empty dev database of this worktree
#   gc                          remove abandoned test servers and the kept
#                               test servers of this worktree
#   destroy --yes               delete the dev server and the dev volume of
#                               this worktree, with all its data
#   sqlx prepare|check          cargo sqlx prepare under a worktree lock
#   e2e                         the Playwright journeys on the test server
set -euo pipefail

# A service port of .env.local or the shell, before this script uses the
# name for anything else.
SERVICE_PORT_SET="${PORT:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
IMAGE="${SPOT_PRICE_PG_IMAGE:?SPOT_PRICE_PG_IMAGE is not set; run the command through mise}"
# The checkout whose servers this command uses. SPOT_PRICE_DB_ROOT selects
# another checkout of this repository explicitly, for example to restore data
# into it; the code, migrations, and build stay those of this checkout.
DB_ROOT="$(cd "${SPOT_PRICE_DB_ROOT:-$ROOT}" && pwd -P)"
# mise also finds the tasks of a parent directory's mise.toml. A checkout
# under .claude/worktrees of another checkout then runs the script of the
# outer checkout; stop before it touches the servers of that checkout.
if [ -n "${MISE_ORIGINAL_CWD:-}" ] && [ -z "${SPOT_PRICE_DB_ROOT:-}" ]; then
  CALLER_ROOT="$(cd "$MISE_ORIGINAL_CWD" && git rev-parse --show-toplevel 2>/dev/null || true)"
  if [ -n "$CALLER_ROOT" ] && [ "$(cd "$CALLER_ROOT" && pwd -P)" != "$ROOT" ]; then
    echo "db: this task comes from $ROOT/mise.toml, but you run it in $CALLER_ROOT, which has no such task" >&2
    exit 1
  fi
fi
WORKTREE_ID="$(printf '%s' "$DB_ROOT" | shasum -a 256 | cut -c1-12)"
STATE="$DB_ROOT/.tmp/db"
# Shared by all worktrees of this repository: the test server slots and the
# markers of kept test servers.
COMMON="$(cd "$ROOT" && cd "$(git rev-parse --git-common-dir)" && pwd -P)/spot-price-db"
DEV_NAME="spot-price-dev-$WORKTREE_ID"
DEV_VOLUME="spot-price-dev-$WORKTREE_ID"
DEV_DATABASE="spot_price"
TEST_DATABASE="spot_price_test"
E2E_DATABASE="spot_price_e2e"
# Host ports for published servers. A port in use fails `container run`;
# the script then tries another one.
PORT_MIN=48000
PORT_SPAN=6000
# Resource limits of one server (one lightweight VM each).
CPUS="${SPOT_PRICE_DB_CPUS:-2}"
MEMORY="${SPOT_PRICE_DB_MEMORY:-1G}"
# At most this many test servers run at the same time on this Mac, over all
# worktrees of this repository. A run waits for a free slot.
MAX_TEST="${SPOT_PRICE_DB_MAX_TEST:-4}"

say() { echo "db: $*" >&2; }
die() {
  say "$*"
  exit 1
}

require_container() {
  command -v container >/dev/null ||
    die "the Apple container tool is missing; install it from https://github.com/apple/container/releases"
  # A sandbox without the XPC service of the tool fails here too.
  container system status >/dev/null 2>&1 ||
    die "the container system service does not answer; run 'container system start', or run outside a sandbox that blocks it"
}

# A process identity that survives PID reuse: PID plus start time. The
# start time text depends on the locale and the time zone, so both are fixed:
# a run of a Finnish terminal and a check of an agent shell must agree.
owner_of() { # $1 = pid
  local started
  started="$(LC_ALL=C TZ=UTC ps -o lstart= -p "$1" 2>/dev/null | tr -s ' ' '_' | sed 's/_$//')" || return 1
  [ -n "$started" ] || return 1
  printf '%s@%s' "$1" "$started"
}

owner_alive() { # $1 = owner token
  local pid="${1%%@*}"
  [ "$(owner_of "$pid" 2>/dev/null || true)" = "$1" ]
}

# A directory lock with owner identity. A lock of a dead owner is stale.
lock() { # $1 = lock directory, $2 = description
  local dir="$1" waited=0 me
  me="$(owner_of $$)"
  mkdir -p "$(dirname "$dir")"
  until mkdir "$dir" 2>/dev/null; do
    if [ -f "$dir/owner" ] && ! owner_alive "$(cat "$dir/owner" 2>/dev/null)"; then
      rm -r -f "${dir:?}"
      continue
    fi
    [ $waited -eq 0 ] && say "waiting for $2"
    waited=$((waited + 1))
    [ $waited -gt 14400 ] && die "timeout after 2 h while waiting for $2 ($dir)"
    sleep 0.5
  done
  echo "$me" >"$dir/owner"
}

unlock() { rm -r -f "${1:?}"; }

# Lines: name<TAB>state<TAB>role<TAB>worktree<TAB>owner<TAB>port<TAB>run
list_servers() {
  container list --all --format json | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    cfg = c.get("configuration", {})
    labels = cfg.get("labels") or {}
    if labels.get("spot-price.role") is None:
        continue
    ports = cfg.get("publishedPorts") or []
    port = ports[0].get("hostPort", "") if ports else ""
    print("\t".join(str(v) for v in (cfg.get("id"), c.get("status", {}).get("state", ""),
        labels.get("spot-price.role", ""), labels.get("spot-price.worktree", ""),
        labels.get("spot-price.owner", ""), port, labels.get("spot-price.run", ""))))
'
}

server_line() { # $1 = name
  list_servers | awk -F '\t' -v n="$1" '$1 == n'
}

# Delete a container and confirm that it is gone. A failed delete is not
# fatal: the server stays visible, and `db:gc` removes it when its owner is
# gone.
remove_server() { # $1 = name
  local attempt
  for attempt in 1 2 3; do
    container delete --force "$1" >/dev/null 2>&1 || true
    container inspect "$1" >/dev/null 2>&1 || return 0
    sleep 1
  done
  say "could not delete $1; mise run db:gc removes it later"
}

# Start a server container on a free host port and set HOST_PORT.
# $1 = name; then extra `container run` arguments, `--`, postgres arguments.
# macOS has bash 3.2: a command substitution does not inherit `set -e`, so
# the functions here return values in globals, not on standard output.
HOST_PORT=""
run_server() {
  local name="$1" port attempt log run_args=() pg_args=()
  shift
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    run_args+=("$1")
    shift
  done
  [ $# -gt 0 ] && shift
  pg_args=("$@")
  log="$STATE/run-$name.log"
  for attempt in $(seq 1 20); do
    port=$((PORT_MIN + RANDOM % PORT_SPAN))
    if container run --detach --progress none --name "$name" --cpus "$CPUS" --memory "$MEMORY" \
      --publish "127.0.0.1:$port:5432" ${run_args[@]+"${run_args[@]}"} \
      "$IMAGE" postgres ${pg_args[@]+"${pg_args[@]}"} >"$log" 2>&1; then
      rm -f "$log"
      HOST_PORT="$port"
      return 0
    fi
    remove_server "$name"
    if ! grep -q "Address already in use" "$log"; then
      cat "$log" >&2
      rm -f "$log"
      die "container run failed for $name"
    fi
  done
  rm -f "$log"
  die "no free host port after 20 attempts"
}

# Wait until the final server accepts TCP connections in the container and
# the published port answers on the host. The image runs a temporary server
# without TCP during the first initialization; it does not count.
wait_ready() { # $1 = name, $2 = port
  local i
  for i in $(seq 1 240); do
    if container exec "$1" pg_isready --quiet --host=127.0.0.1 --username=postgres 2>/dev/null &&
      nc -z 127.0.0.1 "$2" 2>/dev/null; then
      return 0
    fi
    sleep 0.25
  done
  container logs "$1" >&2 2>/dev/null || true
  die "server $1 did not become ready in 60 s"
}

psql_in() { # $1 = name, $2 = database, then psql arguments
  local name="$1" database="$2"
  shift 2
  container exec "$name" psql --quiet --no-psqlrc --username=postgres --dbname="$database" \
    --tuples-only --no-align -v ON_ERROR_STOP=1 "$@"
}

url_for() { # $1 = password, $2 = port, $3 = database
  printf 'postgres://postgres:%s@127.0.0.1:%s/%s' "$1" "$2" "$3"
}

# SQLx and libpq read PG* variables when a URL leaves a setting out. The
# local servers need none of them.
clear_pg_env() {
  unset PGHOST PGHOSTADDR PGPORT PGDATABASE PGUSER PGPASSWORD PGPASSFILE PGSERVICE \
    PGSERVICEFILE PGSSLMODE PGSSLROOTCERT PGSSLCERT PGSSLKEY PGOPTIONS PGAPPNAME
}

# --- dev server ------------------------------------------------------------

dev_password() {
  mkdir -p "$STATE"
  if [ ! -s "$STATE/dev-password" ]; then
    (umask 077 && openssl rand -hex 16 >"$STATE/dev-password")
  fi
  cat "$STATE/dev-password"
}

# Ensure that the dev server of this worktree runs and set HOST_PORT.
dev_ensure() {
  local line state port password
  require_container
  mkdir -p "$STATE"
  lock "$STATE/dev.lock" "the dev server lock of this worktree"
  dev_password >/dev/null
  password="$(cat "$STATE/dev-password")"
  if ! container volume inspect "$DEV_VOLUME" >/dev/null 2>&1; then
    container volume create --label spot-price.role=dev --label "spot-price.worktree=$WORKTREE_ID" \
      --label "spot-price.path=$DB_ROOT" "$DEV_VOLUME" >/dev/null
  fi
  line="$(server_line "$DEV_NAME")"
  state="$(cut -f2 <<<"$line")"
  port="$(cut -f6 <<<"$line")"
  if [ -n "$line" ] && [ "$state" != "running" ]; then
    # The published port can be in use now. Then recreate the container on
    # another port; the data stays in the volume.
    container start "$DEV_NAME" >/dev/null 2>&1 || {
      remove_server "$DEV_NAME"
      line=""
    }
  fi
  if [ -z "$line" ]; then
    # Durable PostgreSQL defaults: no fsync or commit settings here.
    run_server "$DEV_NAME" \
      --volume "$DEV_VOLUME:/var/lib/postgresql" \
      --label spot-price.role=dev --label "spot-price.worktree=$WORKTREE_ID" \
      --label "spot-price.path=$DB_ROOT" \
      --env "POSTGRES_PASSWORD=$password" --env "POSTGRES_DB=$DEV_DATABASE" --
    port="$HOST_PORT"
  fi
  wait_ready "$DEV_NAME" "$port"
  # The password file is the source of truth. The image sets the password
  # only at the first start, so apply it again after a lost file.
  psql_in "$DEV_NAME" postgres --command="ALTER ROLE postgres PASSWORD '$password'" >/dev/null
  unlock "$STATE/dev.lock"
  HOST_PORT="$port"
}

dev_url() { # $1 = port
  url_for "$(cat "$STATE/dev-password")" "$1" "$DEV_DATABASE"
}

# The service port of `mise run dev` of this worktree: kept in a state file,
# so the address stays the same between runs. Another worktree gets another
# port.
APP_PORT=""
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

cmd_start() {
  dev_ensure
  say "dev server ready: $DEV_NAME on 127.0.0.1:$HOST_PORT, database $DEV_DATABASE (mise run db:info)"
}

cmd_stop() {
  require_container
  if [ -n "$(server_line "$DEV_NAME")" ]; then
    container stop "$DEV_NAME" >/dev/null
    say "dev server stopped: $DEV_NAME (the data stays in volume $DEV_VOLUME)"
  else
    say "no dev server for this worktree"
  fi
}

cmd_with_dev_db() {
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "usage: $0 with-dev-db -- CMD..."
  clear_pg_env
  if [ -n "${SPOT_PRICE_DATABASE_URL:-}" ]; then
    # An explicit choice of the owner, for example another database. A
    # DATABASE_URL in .env.local never selects a database by itself.
    say "using SPOT_PRICE_DATABASE_URL, not the dev server of this worktree"
    DATABASE_URL="$SPOT_PRICE_DATABASE_URL" exec "$@"
  fi
  dev_ensure
  say "using the dev server $DEV_NAME on 127.0.0.1:$HOST_PORT, database $DEV_DATABASE"
  # The service address: a PORT and a BETTER_AUTH_URL of .env.local or the
  # shell win. The Origin check compares BETTER_AUTH_URL byte for byte with
  # the origin in the browser, so set both or neither.
  local db_port="$HOST_PORT" app_url
  if [ -n "${SERVICE_PORT_SET:-}" ]; then
    APP_PORT="$SERVICE_PORT_SET"
  else
    dev_app_port
  fi
  app_url="${BETTER_AUTH_URL:-http://localhost:$APP_PORT}"
  say "service: $app_url"
  DATABASE_URL="$(dev_url "$db_port")" PORT="$APP_PORT" BETTER_AUTH_URL="$app_url" exec "$@"
}

cmd_psql() {
  dev_ensure
  exec container exec --interactive --tty "$DEV_NAME" \
    psql --username=postgres --dbname="$DEV_DATABASE" "$@"
}

# The dev database has no table other than the SQLx ledger of an empty
# migration run.
dev_is_empty() {
  local count
  count="$(psql_in "$DEV_NAME" "$DEV_DATABASE" --command="SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'r' AND n.nspname NOT IN ('pg_catalog', 'information_schema')")"
  [ "$count" = "0" ]
}

cmd_dump() { # $1 = file
  local file="${1:-}"
  [ -n "$file" ] || die "usage: $0 dump FILE"
  [ -e "$file" ] && die "$file exists; choose a new file"
  dev_ensure
  container exec "$DEV_NAME" pg_dump --username=postgres --format=custom --dbname="$DEV_DATABASE" >"$file.partial"
  mv "$file.partial" "$file"
  say "dumped $DEV_DATABASE of $DEV_NAME to $file"
}

restore_stream() { # standard input = a pg_dump custom-format archive
  container exec --interactive "$DEV_NAME" pg_restore --username=postgres --dbname="$DEV_DATABASE" \
    --exit-on-error --single-transaction --no-owner --no-privileges
}

cmd_restore() { # $1 = file
  local file="${1:-}"
  [ -f "$file" ] || die "usage: $0 restore FILE (a pg_dump -Fc archive)"
  dev_ensure
  dev_is_empty || die "the dev database $DEV_DATABASE of $DEV_NAME is not empty; restore needs an empty database"
  restore_stream <"$file"
  say "restored $file into $DEV_DATABASE of $DEV_NAME; run mise run dev to migrate it"
}

cmd_clone() { # $1 = source worktree
  local source="${1:-}" source_root source_id source_name
  [ -d "$source" ] || die "usage: $0 clone WORKTREE (the path of another checkout of this repository)"
  source_root="$(cd "$source" && pwd -P)"
  [ "$source_root" != "$DB_ROOT" ] || die "the source is this worktree"
  source_id="$(printf '%s' "$source_root" | shasum -a 256 | cut -c1-12)"
  source_name="spot-price-dev-$source_id"
  [ "$(cut -f2 <<<"$(server_line "$source_name")")" = "running" ] ||
    die "the dev server of $source_root does not run; start it there with mise run db:start"
  dev_ensure
  dev_is_empty || die "the dev database $DEV_DATABASE of $DEV_NAME is not empty; clone needs an empty database"
  container exec "$source_name" pg_dump --username=postgres --format=custom --dbname="$DEV_DATABASE" | restore_stream
  say "copied $DEV_DATABASE of $source_name into $DEV_NAME"
}

cmd_destroy() { # $1 = --yes
  [ "${1:-}" = "--yes" ] ||
    die "destroy deletes the dev database of $DB_ROOT with all its data; run: $0 destroy --yes"
  require_container
  mkdir -p "$STATE"
  lock "$STATE/dev.lock" "the dev server lock of this worktree"
  remove_server "$DEV_NAME"
  if container volume inspect "$DEV_VOLUME" >/dev/null 2>&1; then
    container volume delete "$DEV_VOLUME" >/dev/null
  fi
  rm -f "$STATE/dev-password" "$STATE/app-port"
  unlock "$STATE/dev.lock"
  say "deleted the dev server and volume $DEV_NAME"
}

cmd_info() {
  local line found=0 name state role worktree owner port run
  require_container
  echo "worktree:    $DB_ROOT"
  echo "worktree id: $WORKTREE_ID"
  echo "image:       $IMAGE"
  line="$(server_line "$DEV_NAME")"
  if [ -n "$line" ]; then
    echo "dev server:  $DEV_NAME ($(cut -f2 <<<"$line"))"
    echo "  volume:    $DEV_VOLUME"
    echo "  port:      127.0.0.1:$(cut -f6 <<<"$line")"
    echo "  database:  $DEV_DATABASE (user postgres, password in .tmp/db/dev-password)"
    if [ "$(cut -f2 <<<"$line")" = "running" ]; then
      echo "  version:   $(psql_in "$DEV_NAME" postgres --command='SHOW server_version')"
    fi
  else
    echo "dev server:  none (mise run db:start creates it)"
  fi
  if [ -s "$STATE/app-port" ]; then
    echo "service:     http://localhost:$(cat "$STATE/app-port") (mise run dev)"
  fi
  while IFS=$'\t' read -r name state role worktree owner port run; do
    [ "$role" = "test" ] && [ "$worktree" = "$WORKTREE_ID" ] || continue
    found=1
    echo "test server: $name ($state) on 127.0.0.1:$port"
  done < <(list_servers)
  [ $found -eq 1 ] || echo "test servers: none for this worktree"
}

# --- test servers ----------------------------------------------------------

SLOT=""
TEST_NAME=""
TEST_URL=""

acquire_slot() {
  local i waited=0 me
  me="$(owner_of $$)"
  mkdir -p "$COMMON/slots"
  while :; do
    for i in $(seq 1 "$MAX_TEST"); do
      if mkdir "$COMMON/slots/$i" 2>/dev/null; then
        echo "$me" >"$COMMON/slots/$i/owner"
        SLOT="$COMMON/slots/$i"
        return 0
      fi
      if [ -f "$COMMON/slots/$i/owner" ] && ! owner_alive "$(cat "$COMMON/slots/$i/owner" 2>/dev/null)"; then
        rm -r -f "${COMMON:?}/slots/$i"
      fi
    done
    [ $waited -eq 0 ] && say "waiting for a test server slot (SPOT_PRICE_DB_MAX_TEST=$MAX_TEST)"
    waited=$((waited + 1))
    [ $waited -gt 14400 ] && die "timeout after 2 h while waiting for a test server slot"
    sleep 0.5
  done
}

# Remove test servers whose owner process is gone, in every worktree of this
# Mac. A kept server has a marker and stays. Dev servers never go here.
reap_abandoned() {
  local name state role worktree owner port run
  while IFS=$'\t' read -r name state role worktree owner port run; do
    [ "$role" = "test" ] || continue
    [ -n "$owner" ] && ! owner_alive "$owner" || continue
    [ -e "$COMMON/kept/$name" ] && continue
    say "removing abandoned test server $name"
    remove_server "$name"
  done < <(list_servers)
}

test_cleanup() {
  local status=$?
  trap - EXIT INT TERM HUP
  if [ -n "$TEST_NAME" ]; then
    local keep="${SPOT_PRICE_DB_KEEP:-}"
    if [ "$keep" = "always" ] || { [ "$keep" = "failed" ] && [ $status -ne 0 ]; }; then
      mkdir -p "$COMMON/kept" "$STATE/logs"
      echo "$DB_ROOT" >"$COMMON/kept/$TEST_NAME"
      container logs "$TEST_NAME" >"$STATE/logs/$TEST_NAME.log" 2>&1 || true
      say "kept $TEST_NAME: DATABASE_URL=$TEST_URL"
      say "server log: .tmp/db/logs/$TEST_NAME.log; remove with: mise run db:gc"
    else
      remove_server "$TEST_NAME"
    fi
  fi
  [ -n "$SLOT" ] && unlock "$SLOT"
  exit $status
}

cmd_with_test_db() {
  local share=0 password status
  if [ "${1:-}" = "--share" ]; then
    share=1
    shift
  fi
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "usage: $0 with-test-db [--share] -- CMD..."
  clear_pg_env
  # A nested call reuses the server of the enclosing run only when that run
  # asked for it with --share (verify does) and the server still runs.
  if [ "${SPOT_PRICE_DB_SHARE:-}" = "1" ] && [ -n "${SPOT_PRICE_TEST_DATABASE_URL:-}" ]; then
    require_container
    [ "$(cut -f2 <<<"$(server_line "${SPOT_PRICE_DB_CONTAINER:-}")")" = "running" ] ||
      die "the shared test server ${SPOT_PRICE_DB_CONTAINER:-} of the enclosing run does not run"
    DATABASE_URL="$SPOT_PRICE_TEST_DATABASE_URL" exec "$@"
  fi
  require_container
  mkdir -p "$STATE"
  reap_abandoned
  acquire_slot
  TEST_NAME="spot-price-test-$WORKTREE_ID-$(openssl rand -hex 4)"
  trap test_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
  password="$(openssl rand -hex 16)"
  # Speed settings for disposable data only. The dev server never gets them.
  run_server "$TEST_NAME" \
    --tmpfs /var/lib/postgresql \
    --label spot-price.role=test --label "spot-price.worktree=$WORKTREE_ID" \
    --label "spot-price.owner=$(owner_of $$)" --label "spot-price.run=$TEST_NAME" \
    --env "POSTGRES_PASSWORD=$password" --env "POSTGRES_DB=$TEST_DATABASE" -- \
    -c fsync=off -c synchronous_commit=off -c full_page_writes=off -c max_connections=200
  wait_ready "$TEST_NAME" "$HOST_PORT"
  TEST_URL="$(url_for "$password" "$HOST_PORT" "$TEST_DATABASE")"
  # --no-dotenv: never read a .env file.
  DATABASE_URL="$TEST_URL" sqlx migrate run --no-dotenv --source "$ROOT/migrations" >/dev/null
  say "test server ready: $TEST_NAME on 127.0.0.1:$HOST_PORT"
  status=0
  if [ $share -eq 1 ]; then
    SPOT_PRICE_DB_SHARE=1 SPOT_PRICE_TEST_DATABASE_URL="$TEST_URL" \
      SPOT_PRICE_DB_CONTAINER="$TEST_NAME" DATABASE_URL="$TEST_URL" "$@" || status=$?
  else
    SPOT_PRICE_DB_SHARE="" SPOT_PRICE_TEST_DATABASE_URL="" \
      SPOT_PRICE_DB_CONTAINER="$TEST_NAME" DATABASE_URL="$TEST_URL" "$@" || status=$?
  fi
  exit $status
}

cmd_gc() {
  local name state role worktree owner port run marker
  require_container
  reap_abandoned
  while IFS=$'\t' read -r name state role worktree owner port run; do
    [ "$role" = "test" ] || continue
    [ "$worktree" = "$WORKTREE_ID" ] && [ -e "$COMMON/kept/$name" ] || continue
    say "removing kept test server $name"
    remove_server "$name"
    rm -f "$COMMON/kept/$name"
  done < <(list_servers)
  # Markers of servers that no longer exist.
  for marker in "$COMMON"/kept/*; do
    [ -e "$marker" ] || continue
    [ -n "$(server_line "$(basename "$marker")")" ] || rm -f "$marker"
  done
  # Dev servers of checkouts that no longer exist. Their volumes hold data,
  # so they are only listed.
  while IFS=$'\t' read -r name state role worktree owner port run; do
    [ "$role" = "dev" ] || continue
    local path
    path="$(container list --all --format json | python3 -c '
import json, sys
for c in json.load(sys.stdin):
    cfg = c.get("configuration", {})
    if cfg.get("id") == sys.argv[1]:
        print((cfg.get("labels") or {}).get("spot-price.path", ""))
' "$name")"
    [ -n "$path" ] && [ ! -d "$path" ] || continue
    say "dev server $name belongs to a removed checkout ($path); it and volume $name stay until you delete them"
  done < <(list_servers)
}

# `cargo sqlx prepare` of this worktree, one at a time: prepare rewrites
# .sqlx, and both modes share the SQLx work files under target/. Runs inside
# with-test-db, which sets DATABASE_URL.
cmd_sqlx() { # $1 = prepare | check
  local check=()
  case "${1:-}" in
    prepare) ;;
    check) check=(--check) ;;
    *) die "usage: $0 sqlx prepare|check" ;;
  esac
  [ -n "${SPOT_PRICE_DB_CONTAINER:-}" ] || die "run sqlx through with-test-db"
  mkdir -p "$STATE"
  lock "$STATE/sqlx.lock" "the SQLx metadata lock of this worktree"
  trap 'unlock "$STATE/sqlx.lock"' EXIT
  cargo sqlx prepare --no-dotenv --workspace ${check[@]+"${check[@]}"} -- --all-targets --locked
}

# The Playwright journeys (playwright.config.ts). Runs inside with-test-db:
# the e2e database is on the test server of this run. The run has its own
# copy of the release binary, service port, and output directory, so two runs
# in one worktree do not share files.
cmd_e2e() {
  local container_name="${SPOT_PRICE_DB_CONTAINER:-}" run_dir url status port
  [ -n "$container_name" ] && [ -n "${DATABASE_URL:-}" ] || die "run e2e through with-test-db"
  run_dir="$ROOT/.tmp/e2e/$container_name"
  mkdir -p "$run_dir"
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

cmd_setup() {
  local version
  command -v container >/dev/null ||
    die "the Apple container tool is missing; install it from https://github.com/apple/container/releases"
  version="$(container --version | sed -E 's/.*version ([0-9.]+).*/\1/')"
  case "$version" in
    0.*) die "container $version is too old; version 1.0.0 or later is required" ;;
  esac
  say "container $version"
  container system status >/dev/null 2>&1 ||
    die "the container system service is not running; run: container system start, then run setup again"
  # Only the arm64 image: a pull of every platform takes minutes.
  container image pull --platform linux/arm64 --progress none "$IMAGE" >/dev/null
  say "image ready: $IMAGE"
}

case "${1:-}" in
  setup) cmd_setup ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  info) cmd_info ;;
  psql)
    shift
    cmd_psql "$@"
    ;;
  with-dev-db)
    shift
    cmd_with_dev_db "$@"
    ;;
  with-test-db)
    shift
    cmd_with_test_db "$@"
    ;;
  dump) cmd_dump "${2:-}" ;;
  restore) cmd_restore "${2:-}" ;;
  clone) cmd_clone "${2:-}" ;;
  gc) cmd_gc ;;
  destroy) cmd_destroy "${2:-}" ;;
  sqlx)
    shift
    cmd_sqlx "$@"
    ;;
  e2e) cmd_e2e ;;
  *)
    sed -n '2,/^set -euo/p' "$0" | sed '$d' >&2
    exit 2
    ;;
esac

#!/usr/bin/env bash
# Local PostgreSQL in Apple containers.
#
# This file is the same in caltodo, mastofeed, and spot-price. Keep it
# byte-identical: put every project difference in scripts/db-project.sh.
#
# The Mac has no PostgreSQL install. Each database server is a container of
# the image in <ENV>_PG_IMAGE (mise.toml). There are two kinds:
#
# - Dev server: one per worktree. It keeps its data in a named volume and uses
#   the durable PostgreSQL defaults. A command that needs it (`dev`, `db:psql`,
#   ...) starts it, and the last such command stops it at exit. `db:start`
#   keeps it running until `db:stop`.
# - Test server: one per independent run of a test task. Its data is in
#   memory (tmpfs) with fsync off. The run removes it at the end.
#
# Every command that starts a server first cleans up: it removes the test
# servers of runs that ended, and the dev server and dev volume of a worktree
# that git no longer knows and whose directory is gone.
#
# The worktree id is a hash of the physical checkout path, so two worktrees
# never share a container, even on the same branch. A test server name adds a
# random run id. Ports are published on 127.0.0.1 only. Every connection
# needs the password of its server.
#
# Usage: scripts/db.sh <command>
#   setup                       check the container tool, fetch the image
#   start | stop                keep the dev server running / stop it
#   info | psql                 show the servers / psql on the dev database
#   with-dev-db -- CMD...       run CMD with DATABASE_URL of the dev server
#   with-test-db [--share] -- CMD...
#                               run CMD with DATABASE_URL of a new test server
#   dump FILE                   write the dev database to FILE (pg_dump -Fc)
#   restore FILE                replace the dev database with FILE
#   clone WORKTREE              replace the dev database with the dev
#                               database of another worktree
#   reset                       replace the dev database with an empty one
#   gc                          the clean-up, and remove the kept test servers
#                               of this worktree
#   sqlx prepare|check          cargo sqlx prepare under a worktree lock
#   (restore, clone, and reset first write the old database to .tmp/db/backups)
set -euo pipefail

SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$SCRIPTS/.." && pwd -P)"

# The project settings: DB_PROJECT (names and labels), DB_ENV (the prefix of
# the environment variables), DB_DEV_DATABASE, DB_TEST_DATABASE,
# DB_PORT_MIN, DB_PORT_SPAN, DB_TLS, DB_PROJECT_COMMANDS, and the optional
# hooks project_dev_env, project_info, and project_command.
DB_TLS=0
# shellcheck source=scripts/db-project.sh
. "$SCRIPTS/db-project.sh"

# The value of <DB_ENV>_<name>, or $2.
env_value() { # $1 = name, $2 = default
  local name="${DB_ENV}_$1"
  printf '%s' "${!name:-${2:-}}"
}

IMAGE="$(env_value PG_IMAGE)"
[ -n "$IMAGE" ] || { echo "db: ${DB_ENV}_PG_IMAGE is not set; run the command through mise" >&2; exit 1; }
# The major version of the image tag: postgres:18.7@sha256:... -> 18.
IMAGE_TAG="${IMAGE%%@*}"
IMAGE_TAG="${IMAGE_TAG##*:}"
MAJOR="${IMAGE_TAG%%[.-]*}"
case "$MAJOR" in '' | *[!0-9]*)
  echo "db: ${DB_ENV}_PG_IMAGE needs a tag that starts with the major version" >&2
  exit 1
  ;;
esac

# The checkout whose servers this command uses. <ENV>_DB_ROOT selects another
# checkout of this repository explicitly, for example to restore data into
# it; the code, migrations, and build stay those of this checkout.
DB_ROOT="$(cd "$(env_value DB_ROOT "$ROOT")" && pwd -P)"
# mise also finds the tasks of a parent directory's mise.toml. A checkout
# under .claude/worktrees of another checkout then runs the script of the
# outer checkout; stop before it touches the servers of that checkout.
if [ -n "${MISE_ORIGINAL_CWD:-}" ] && [ -z "$(env_value DB_ROOT)" ]; then
  CALLER_ROOT="$(cd "$MISE_ORIGINAL_CWD" && git rev-parse --show-toplevel 2>/dev/null || true)"
  if [ -n "$CALLER_ROOT" ] && [ "$(cd "$CALLER_ROOT" && pwd -P)" != "$ROOT" ]; then
    echo "db: this task comes from $ROOT/mise.toml, but you run it in $CALLER_ROOT, which has no such task" >&2
    exit 1
  fi
fi

worktree_id() { printf '%s' "$1" | shasum -a 256 | cut -c1-12; }
WORKTREE_ID="$(worktree_id "$DB_ROOT")"
STATE="$DB_ROOT/.tmp/db"
# Shared by all worktrees of this repository: the test server slots and the
# markers of kept test servers.
COMMON="$(cd "$ROOT" && cd "$(git rev-parse --git-common-dir)" && pwd -P)/$DB_PROJECT-db"
# Test servers of another clone of this repository have other markers, so
# the clean-up leaves the kept servers of that clone alone.
REPO_ID="$(worktree_id "$COMMON")"
DEV_NAME="$DB_PROJECT-dev-$WORKTREE_ID"
# The data directory of one major version does not start on another, so the
# major version names the volume. A volume without the suffix comes from an
# earlier version of this script and holds major version 18.
DEV_VOLUME="$DB_PROJECT-dev-$WORKTREE_ID-pg$MAJOR"
dev_volume_resolve() {
  if [ "$MAJOR" = 18 ] && ! container volume inspect "$DEV_VOLUME" >/dev/null 2>&1 &&
    container volume inspect "$DB_PROJECT-dev-$WORKTREE_ID" >/dev/null 2>&1; then
    DEV_VOLUME="$DB_PROJECT-dev-$WORKTREE_ID"
  fi
}
# Resource limits of one server (one lightweight VM each).
CPUS="$(env_value DB_CPUS 2)"
MEMORY="$(env_value DB_MEMORY 1G)"
# At most this many test servers run at the same time on this Mac, over all
# worktrees of this repository. A run waits for a free slot.
MAX_TEST="$(env_value DB_MAX_TEST 4)"
# A lock or a slot wait stops after 2 hours (0.5 s steps). One verify can
# take longer than 10 minutes on a loaded Mac.
WAIT_LIMIT=14400
# TLS (DB_TLS=1): a self-signed X.509 v3 certificate made in the container.
# A key from a bind mount would have the owner and mode of macOS; PostgreSQL
# requires a key that only its user can read.
TLS_DIR=/var/lib/postgresql-tls
PG_TLS_ARGS=(-c ssl=on -c "ssl_cert_file=$TLS_DIR/server.crt" -c "ssl_key_file=$TLS_DIR/server.key")

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

# --- processes and locks ---------------------------------------------------

# A process identity that survives PID reuse: PID plus start time. The start
# time text depends on the locale and the time zone, so both are fixed: a run
# of a Finnish terminal and a check of an agent shell must agree.
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

# A lock or slot directory is stale when its owner is gone, or when it has no
# owner file a minute after its creation (a crash between mkdir and the write
# of the owner).
dir_stale() { # $1 = directory
  if [ -f "$1/owner" ]; then
    ! owner_alive "$(cat "$1/owner" 2>/dev/null)"
  else
    [ -n "$(find "$1" -maxdepth 0 -mmin +1 2>/dev/null)" ]
  fi
}

# A directory lock with owner identity (macOS has no flock(1)). A stale lock
# is removed. Two waiters that see the same stale lock at the same moment can
# both take it; the window is short.
lock() { # $1 = lock directory, $2 = description
  local dir="$1" waited=0 me
  me="$(owner_of $$)"
  mkdir -p "$(dirname "$dir")"
  until mkdir "$dir" 2>/dev/null; do
    if dir_stale "$dir"; then
      rm -r -f "${dir:?}"
      continue
    fi
    [ $waited -eq 0 ] && say "waiting for $2"
    waited=$((waited + 1))
    [ $waited -gt $WAIT_LIMIT ] && die "timeout after 2 h while waiting for $2 ($dir)"
    sleep 0.5
  done
  echo "$me" >"$dir/owner"
}

unlock() { rm -r -f "${1:?}"; }

# --- containers ------------------------------------------------------------

# Lines: name state role worktree owner port run repo path major, separated
# by tabs. An empty field is "-": `read` merges adjacent tabs.
list_servers() {
  container list --all --format json | python3 -c '
import json, sys
p = sys.argv[1] + "."
for c in json.load(sys.stdin):
    cfg = c.get("configuration", {})
    labels = cfg.get("labels") or {}
    if labels.get(p + "role") is None:
        continue
    ports = [x for x in (cfg.get("publishedPorts") or []) if x.get("containerPort") == 5432]
    port = ports[0].get("hostPort", "") if ports else ""
    print("\t".join(str(v) or "-" for v in (cfg.get("id"), c.get("status", {}).get("state", ""),
        labels.get(p + "role", ""), labels.get(p + "worktree", ""), labels.get(p + "owner", ""),
        port, labels.get(p + "run", ""), labels.get(p + "repo", ""), labels.get(p + "path", ""),
        labels.get(p + "major", ""))))
' "$DB_PROJECT"
}

# Lines: name role worktree owner path, separated by tabs, for each volume
# with a role label of this project. An empty field is "-".
list_volumes() {
  container volume list --format json | python3 -c '
import json, sys
p = sys.argv[1] + "."
for v in json.load(sys.stdin):
    cfg = v.get("configuration", v)
    labels = cfg.get("labels") or {}
    if labels.get(p + "role") is None:
        continue
    print("\t".join(str(x) or "-" for x in (cfg.get("name"), labels.get(p + "role", ""),
        labels.get(p + "worktree", ""), labels.get(p + "owner", ""), labels.get(p + "path", ""))))
' "$DB_PROJECT"
}

server_line() { # $1 = name
  list_servers | awk -F '\t' -v n="$1" '$1 == n'
}

field() { # $1 = line, $2 = field number; "-" becomes empty
  local value
  value="$(cut -f"$2" <<<"$1")"
  [ "$value" = "-" ] || printf '%s' "$value"
}

# Delete a container and confirm that it is gone. A failed delete is not
# fatal: the next clean-up tries again.
remove_server() { # $1 = name
  local attempt
  for attempt in 1 2 3; do
    container delete --force "$1" >/dev/null 2>&1 || true
    container inspect "$1" >/dev/null 2>&1 || return 0
    sleep 1
  done
  say "could not delete $1; the next clean-up tries again"
}

# The container command with TLS: generate the key pair in the container,
# outside the data volume, then start the image entrypoint.
tls_command() {
  # `req -x509` writes a version 3 certificate. rustls rejects older versions.
  printf '%s' "set -e
mkdir -p $TLS_DIR
if [ ! -f $TLS_DIR/server.key ]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj /CN=127.0.0.1 \
    -keyout $TLS_DIR/server.key -out $TLS_DIR/server.crt >/dev/null 2>&1
fi
chown -R postgres:postgres $TLS_DIR
chmod 600 $TLS_DIR/server.key
exec docker-entrypoint.sh \"\$@\""
}

# Start a server container on a free host port and set SERVER_PORT.
# $1 = name; then extra `container run` arguments, `--`, postgres arguments.
# macOS has bash 3.2: a command substitution does not inherit `set -e`, so
# the functions here return values in globals, not on standard output.
SERVER_PORT=""
run_server() {
  local name="$1" port attempt log run_args=() pg_args=() image_args=()
  shift
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    run_args+=("$1")
    shift
  done
  [ $# -gt 0 ] && shift
  pg_args=("$@")
  if [ "$DB_TLS" = 1 ]; then
    image_args=(--entrypoint /bin/sh "$IMAGE" -c "$(tls_command)" postgres postgres "${PG_TLS_ARGS[@]}")
  else
    image_args=("$IMAGE" postgres)
  fi
  mkdir -p "$STATE"
  log="$STATE/run-$name.log"
  for attempt in $(seq 1 20); do
    port=$((DB_PORT_MIN + RANDOM % DB_PORT_SPAN))
    if container run --detach --progress none --name "$name" --cpus "$CPUS" --memory "$MEMORY" \
      --publish "127.0.0.1:$port:5432" ${run_args[@]+"${run_args[@]}"} \
      "${image_args[@]}" ${pg_args[@]+"${pg_args[@]}"} >"$log" 2>&1; then
      rm -f "$log"
      SERVER_PORT="$port"
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

# PostgreSQL answers an SSLRequest with one byte, N or S. An open TCP port is
# not enough: after a change of the container network, the published port of
# an older container accepts connections but never answers.
host_answers() { # $1 = port
  [ -n "$1" ] || return 1
  python3 - "$1" <<'PY' 2>/dev/null
import socket, struct, sys
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=2)
s.settimeout(2)
s.sendall(struct.pack("!ii", 8, 80877103))
sys.exit(0 if s.recv(1) in (b"N", b"S") else 1)
PY
}

ready_inside() { # $1 = name
  container exec "$1" pg_isready --quiet --host=127.0.0.1 --username=postgres 2>/dev/null
}

# Wait until the final server accepts TCP connections in the container and
# the published port answers on the host. The image runs a temporary server
# without TCP during the first initialization; it does not count.
wait_ready() { # $1 = name, $2 = port
  local i
  for i in $(seq 1 240); do
    if ready_inside "$1" && host_answers "$2"; then
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

# SQLx and libpq read PG* variables when a URL leaves a setting out. The local
# servers need none of them.
clear_pg_env() {
  unset PGHOST PGHOSTADDR PGPORT PGDATABASE PGUSER PGPASSWORD PGPASSFILE PGSERVICE \
    PGSERVICEFILE PGSSLMODE PGSSLROOTCERT PGSSLCERT PGSSLKEY PGOPTIONS PGAPPNAME
}

# --- clean-up --------------------------------------------------------------

# A worktree is gone when git of this repository no longer lists it and its
# directory is gone, but its parent directory still exists. A missing parent
# (an unmounted disk, a removed temporary directory tree) keeps the data.
worktree_gone() { # $1 = path
  local path="$1"
  case "$path" in /*) ;; *) return 1 ;; esac
  [ ! -e "$path" ] && [ -d "$(dirname "$path")" ] || return 1
  ! git -C "$ROOT" worktree list --porcelain 2>/dev/null | grep -qxF "worktree $path"
}

# Remove the test servers of runs that ended (a kept server stays), and the
# dev server and dev volumes of worktrees that are gone.
clean_up_servers() {
  local name state role worktree owner port run repo path major gone_ids=" "
  while IFS=$'\t' read -r name state role worktree owner port run repo path major; do
    if [ "$role" = dev ]; then
      worktree_gone "$path" || continue
      say "removing the dev server $name (worktree $path is gone)"
      remove_server "$name"
      gone_ids="$gone_ids${name#"$DB_PROJECT"-dev-} "
      continue
    fi
    # A test or other run server: it has an owner process.
    [ "$owner" != "-" ] && ! owner_alive "$owner" || continue
    [ "$repo" = "$REPO_ID" ] || [ "$repo" = "-" ] || continue
    [ -e "$COMMON/kept/$name" ] && continue
    say "removing the abandoned $role server $name"
    remove_server "$name"
    container volume delete "$name" >/dev/null 2>&1 || true
  done < <(list_servers)
  while IFS=$'\t' read -r name role worktree owner path; do
    if [ "$role" = dev ]; then
      # A volume of an earlier version of this script can lack the path
      # label; its name carries the worktree id.
      worktree="${name#"$DB_PROJECT"-dev-}"
      worktree="${worktree%%-pg*}"
      worktree_gone "$path" || case "$gone_ids" in *" $worktree "*) ;; *) continue ;; esac
      say "removing the dev volume $name and its data (worktree ${path#-} is gone)"
      container volume delete "$name" >/dev/null 2>&1 || true
      continue
    fi
    [ "$owner" != "-" ] && ! owner_alive "$owner" || continue
    [ -e "$COMMON/kept/$name" ] && continue
    [ -n "$(server_line "$name")" ] && continue
    say "removing the abandoned $role volume $name"
    container volume delete "$name" >/dev/null 2>&1 || true
  done < <(list_volumes)
}

# --- dev server ------------------------------------------------------------

dev_password() {
  mkdir -p "$STATE"
  if [ ! -s "$STATE/dev-password" ]; then
    (umask 077 && openssl rand -hex 16 >"$STATE/dev-password")
  fi
  cat "$STATE/dev-password"
}

dev_labels() {
  DEV_LABELS=(--label "$DB_PROJECT.role=dev" --label "$DB_PROJECT.worktree=$WORKTREE_ID"
    --label "$DB_PROJECT.path=$DB_ROOT" --label "$DB_PROJECT.repo=$REPO_ID"
    --label "$DB_PROJECT.major=$MAJOR")
}

# Ensure that the dev server of this worktree runs and set SERVER_PORT. The
# caller holds the dev lock.
dev_ensure_locked() {
  local line state port password major
  dev_volume_resolve
  dev_password >/dev/null
  password="$(cat "$STATE/dev-password")"
  dev_labels
  if ! container volume inspect "$DEV_VOLUME" >/dev/null 2>&1; then
    container volume create "${DEV_LABELS[@]}" "$DEV_VOLUME" >/dev/null
  fi
  line="$(server_line "$DEV_NAME")"
  state="$(field "$line" 2)"
  port="$(field "$line" 6)"
  major="$(field "$line" 10)"
  # A container of another major version is replaced; the volume of that
  # version stays, and this version uses its own volume. A container without
  # the label comes from an earlier version of this script (major 18).
  if [ -n "$line" ] && [ "${major:-18}" != "$MAJOR" ]; then
    say "replacing $DEV_NAME (PostgreSQL ${major:-18}); its volume stays"
    remove_server "$DEV_NAME"
    line=""
  fi
  if [ "$state" = running ] && ! host_answers "$port"; then
    # The container runs, but PostgreSQL does not answer through the
    # published port: the port of an older container after a change of the
    # container network, or a PostgreSQL that stopped inside a running
    # container. Every start holds this lock and waits until the server is
    # ready, so no start is in progress. Restart the container; the data
    # stays in the volume.
    say "$DEV_NAME runs but does not answer; restarting the container"
    container stop "$DEV_NAME" >/dev/null 2>&1 || true
    state=stopped
  fi
  if [ -n "$line" ] && [ "$state" != running ]; then
    # The published port can be in use now. Then create the container again
    # on another port; the data stays in the volume.
    container start "$DEV_NAME" >/dev/null 2>&1 || {
      remove_server "$DEV_NAME"
      line=""
    }
  fi
  if [ -z "$line" ]; then
    # Durable PostgreSQL defaults: no fsync or commit settings here.
    run_server "$DEV_NAME" --volume "$DEV_VOLUME:/var/lib/postgresql" "${DEV_LABELS[@]}" \
      --env "POSTGRES_PASSWORD=$password" --env "POSTGRES_DB=$DB_DEV_DATABASE" --
    port="$SERVER_PORT"
  fi
  wait_ready "$DEV_NAME" "$port"
  # The password file is the source of truth. The image sets the password
  # only at the first start, so apply it again after a lost file.
  psql_in "$DEV_NAME" postgres --command="ALTER ROLE postgres PASSWORD '$password'" >/dev/null
  SERVER_PORT="$port"
}

# The users of the dev server: one file per command that needs it. The last
# user stops the server at exit, unless `db:start` pinned it.
DEV_USER=""

dev_other_users() {
  local file
  for file in "$STATE"/dev-users/*; do
    [ -f "$file" ] && [ "$file" != "$DEV_USER" ] || continue
    if owner_alive "$(cat "$file" 2>/dev/null)"; then
      return 0
    fi
    rm -f "$file"
  done
  return 1
}

dev_release() {
  [ -n "$DEV_USER" ] || return 0
  lock "$STATE/dev.lock" "the dev server lock of this worktree"
  if [ ! -e "$STATE/dev-pinned" ] && ! dev_other_users &&
    [ "$(field "$(server_line "$DEV_NAME")" 2)" = running ]; then
    container stop "$DEV_NAME" >/dev/null 2>&1 || true
    say "dev server stopped: $DEV_NAME (the data stays in volume $DEV_VOLUME)"
  fi
  rm -f "$DEV_USER"
  DEV_USER=""
  unlock "$STATE/dev.lock"
}

dev_exit() {
  local status=$?
  trap - EXIT
  dev_release || true
  exit $status
}

# Start the dev server for this command and register the command as a user.
# The EXIT trap releases the server. The command runs in the foreground, so
# an interactive program keeps its terminal.
dev_acquire() {
  require_container
  mkdir -p "$STATE/dev-users"
  clean_up_servers
  lock "$STATE/dev.lock" "the dev server lock of this worktree"
  DEV_USER="$STATE/dev-users/$$"
  owner_of $$ >"$DEV_USER"
  # A failed start releases the lock and the user file; the next command
  # finds the server in any state and repairs it.
  trap 'rm -f "$DEV_USER"; unlock "$STATE/dev.lock"' EXIT
  dev_ensure_locked
  unlock "$STATE/dev.lock"
  trap dev_exit EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
}

dev_url() { # $1 = port
  url_for "$(cat "$STATE/dev-password")" "$1" "$DB_DEV_DATABASE"
}

cmd_start() {
  require_container
  mkdir -p "$STATE"
  clean_up_servers
  lock "$STATE/dev.lock" "the dev server lock of this worktree"
  dev_ensure_locked
  : >"$STATE/dev-pinned"
  unlock "$STATE/dev.lock"
  say "dev server ready: $DEV_NAME on 127.0.0.1:$SERVER_PORT, database $DB_DEV_DATABASE; it runs until mise run db:stop"
}

cmd_stop() {
  require_container
  mkdir -p "$STATE"
  lock "$STATE/dev.lock" "the dev server lock of this worktree"
  rm -f "$STATE/dev-pinned"
  if dev_other_users; then
    say "the dev server keeps running for another command of this worktree; that command stops it at exit"
  elif [ -n "$(server_line "$DEV_NAME")" ]; then
    container stop "$DEV_NAME" >/dev/null 2>&1 || true
    say "dev server stopped: $DEV_NAME (the data stays in volume $DEV_VOLUME)"
  else
    say "no dev server for this worktree"
  fi
  unlock "$STATE/dev.lock"
}

cmd_with_dev_db() {
  local status=0
  [ "${1:-}" = "--" ] && shift
  [ $# -gt 0 ] || die "usage: $0 with-dev-db -- CMD..."
  clear_pg_env
  local override
  override="$(env_value DATABASE_URL)"
  if [ -n "$override" ]; then
    # An explicit choice of the owner, for example another database. A
    # DATABASE_URL in .env or .env.local never selects a database by itself.
    say "using ${DB_ENV}_DATABASE_URL, not the dev server of this worktree"
    DATABASE_URL="$override" exec "$@"
  fi
  dev_acquire
  say "using the dev server $DEV_NAME on 127.0.0.1:$SERVER_PORT, database $DB_DEV_DATABASE"
  export DATABASE_URL
  DATABASE_URL="$(dev_url "$SERVER_PORT")"
  if declare -F project_dev_env >/dev/null; then project_dev_env; fi
  "$@" || status=$?
  exit $status
}

cmd_psql() {
  local status=0 terminal=(--interactive)
  # A TTY only for a terminal, so `db:psql -- -c 'SQL'` also works in scripts.
  [ -t 0 ] && [ -t 1 ] && terminal=(--interactive --tty)
  dev_acquire
  container exec "${terminal[@]}" "$DEV_NAME" \
    psql --username=postgres --dbname="$DB_DEV_DATABASE" "$@" || status=$?
  exit $status
}

# The dev database has no table in any schema other than the system schemas.
dev_is_empty() {
  local count
  count="$(psql_in "$DEV_NAME" "$DB_DEV_DATABASE" --command="SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'r' AND n.nspname NOT IN ('pg_catalog', 'information_schema')")"
  [ "$count" = "0" ]
}

# Explicit returns: a caller in an `||` or `if` context turns `set -e` off
# inside this function.
dump_to() { # $1 = container, $2 = file
  container exec "$1" pg_dump --username=postgres --format=custom --dbname="$DB_DEV_DATABASE" >"$2.partial" || {
    rm -f "$2.partial"
    return 1
  }
  mv "$2.partial" "$2"
}

# Write a non-empty dev database to .tmp/db/backups, then replace it with an
# empty database.
dev_replace() {
  local backup
  if ! dev_is_empty; then
    mkdir -p "$STATE/backups"
    backup="$STATE/backups/$DB_DEV_DATABASE-$(date +%Y%m%dT%H%M%S).dump"
    dump_to "$DEV_NAME" "$backup"
    say "wrote the old dev database to ${backup#"$DB_ROOT"/}"
  fi
  psql_in "$DEV_NAME" postgres \
    --command="DROP DATABASE IF EXISTS $DB_DEV_DATABASE WITH (FORCE)" \
    --command="CREATE DATABASE $DB_DEV_DATABASE"
}

restore_stream() { # standard input = a pg_dump custom-format archive
  container exec --interactive "$DEV_NAME" pg_restore --username=postgres --dbname="$DB_DEV_DATABASE" \
    --exit-on-error --single-transaction --no-owner --no-privileges
}

cmd_dump() { # $1 = file
  local file="${1:-}"
  [ -n "$file" ] || die "usage: $0 dump FILE"
  [ -e "$file" ] && die "$file exists; choose a new file"
  dev_acquire
  dump_to "$DEV_NAME" "$file"
  say "dumped $DB_DEV_DATABASE of $DEV_NAME to $file"
}

cmd_restore() { # $1 = file
  local file="${1:-}"
  [ -f "$file" ] || die "usage: $0 restore FILE (a pg_dump -Fc archive)"
  dev_acquire
  dev_replace
  restore_stream <"$file"
  say "restored $file into $DB_DEV_DATABASE of $DEV_NAME; the next dev run migrates it"
}

cmd_clone() { # $1 = source worktree
  local source="${1:-}" source_root source_name source_state file started=0
  [ -d "$source" ] || die "usage: $0 clone WORKTREE (the path of another checkout of this repository)"
  source_root="$(cd "$source" && pwd -P)"
  [ "$source_root" != "$DB_ROOT" ] || die "the source is this worktree"
  source_name="$DB_PROJECT-dev-$(worktree_id "$source_root")"
  source_state="$(field "$(server_line "$source_name")" 2)"
  [ -n "$source_state" ] || die "$source_root has no dev server"
  dev_acquire
  if [ "$source_state" != running ]; then
    container start "$source_name" >/dev/null
    started=1
    for _ in $(seq 1 240); do ready_inside "$source_name" && break || sleep 0.25; done
  fi
  mkdir -p "$STATE"
  file="$STATE/clone-$$.dump"
  dump_to "$source_name" "$file" || {
    [ $started -eq 0 ] || container stop "$source_name" >/dev/null 2>&1 || true
    die "pg_dump of $source_name failed"
  }
  [ $started -eq 0 ] || container stop "$source_name" >/dev/null 2>&1 || true
  dev_replace
  restore_stream <"$file"
  rm -f "$file"
  say "copied $DB_DEV_DATABASE of $source_name into $DEV_NAME; the next dev run migrates it"
}

cmd_reset() {
  dev_acquire
  dev_replace
  say "the dev database $DB_DEV_DATABASE of $DEV_NAME is empty; the next dev run migrates it"
}

cmd_info() {
  local line found=0 name state role worktree owner port run repo path major
  require_container
  dev_volume_resolve
  echo "worktree:    $DB_ROOT"
  echo "worktree id: $WORKTREE_ID"
  echo "image:       $IMAGE"
  line="$(server_line "$DEV_NAME")"
  if [ -n "$line" ]; then
    echo "dev server:  $DEV_NAME ($(field "$line" 2)$([ -e "$STATE/dev-pinned" ] && echo ", kept running by db:start"))"
    echo "  volume:    $DEV_VOLUME"
    echo "  port:      127.0.0.1:$(field "$line" 6)"
    echo "  database:  $DB_DEV_DATABASE (user postgres, password in .tmp/db/dev-password)"
    if [ "$(field "$line" 2)" = running ]; then
      echo "  version:   $(psql_in "$DEV_NAME" postgres --command='SHOW server_version')"
      [ "$DB_TLS" = 1 ] && echo "  ssl:       $(psql_in "$DEV_NAME" postgres --command='SHOW ssl')"
    fi
  else
    echo "dev server:  none (the first command that needs it creates it)"
  fi
  if declare -F project_info >/dev/null; then project_info; fi
  while IFS=$'\t' read -r name state role worktree owner port run repo path major; do
    [ "$role" != dev ] && [ "$worktree" = "$WORKTREE_ID" ] || continue
    found=1
    echo "$role server: $name ($state) on 127.0.0.1:$port"
  done < <(list_servers)
  [ $found -eq 1 ] || echo "test servers: none for this worktree"
}

# --- test servers ----------------------------------------------------------

SLOT=""
TEST_NAME=""
TEST_URL=""
CHILD=""

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
      if [ -d "$COMMON/slots/$i" ] && dir_stale "$COMMON/slots/$i"; then
        rm -r -f "${COMMON:?}/slots/$i"
      fi
    done
    [ $waited -eq 0 ] && say "waiting for a test server slot (${DB_ENV}_DB_MAX_TEST=$MAX_TEST)"
    waited=$((waited + 1))
    [ $waited -gt $WAIT_LIMIT ] && die "timeout after 2 h while waiting for a test server slot"
    sleep 0.5
  done
}

test_exit() {
  local status=$?
  trap - EXIT INT TERM HUP
  # A signal ends the run: stop the command and all its descendants (its own
  # process group) before its server goes away.
  if [ -n "$CHILD" ] && kill -0 "$CHILD" 2>/dev/null; then
    kill -TERM -- "-$CHILD" 2>/dev/null || kill -TERM "$CHILD" 2>/dev/null || true
    wait "$CHILD" 2>/dev/null || true
  fi
  if [ -n "$TEST_NAME" ]; then
    local keep
    keep="$(env_value DB_KEEP)"
    if [ "$keep" = always ] || { [ "$keep" = failed ] && [ $status -ne 0 ]; }; then
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
  # asked for it with --share and the server still runs.
  if [ "$(env_value DB_SHARE)" = 1 ] && [ -n "$(env_value TEST_DATABASE_URL)" ]; then
    require_container
    [ "$(field "$(server_line "$(env_value DB_CONTAINER)")" 2)" = running ] ||
      die "the shared test server $(env_value DB_CONTAINER) of the enclosing run does not run"
    DATABASE_URL="$(env_value TEST_DATABASE_URL)" exec "$@"
  fi
  require_container
  mkdir -p "$STATE"
  clean_up_servers
  acquire_slot
  TEST_NAME="$DB_PROJECT-test-$WORKTREE_ID-$(openssl rand -hex 4)"
  trap test_exit EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
  password="$(openssl rand -hex 16)"
  # Speed settings for disposable data only. The dev server never gets them.
  run_server "$TEST_NAME" \
    --tmpfs /var/lib/postgresql \
    --label "$DB_PROJECT.role=test" --label "$DB_PROJECT.worktree=$WORKTREE_ID" \
    --label "$DB_PROJECT.owner=$(owner_of $$)" --label "$DB_PROJECT.run=$TEST_NAME" \
    --label "$DB_PROJECT.repo=$REPO_ID" \
    --env "POSTGRES_PASSWORD=$password" --env "POSTGRES_DB=$DB_TEST_DATABASE" -- \
    -c fsync=off -c synchronous_commit=off -c full_page_writes=off -c max_connections=200
  wait_ready "$TEST_NAME" "$SERVER_PORT"
  TEST_URL="$(url_for "$password" "$SERVER_PORT" "$DB_TEST_DATABASE")"
  # --no-dotenv: never read a .env file.
  DATABASE_URL="$TEST_URL" sqlx migrate run --no-dotenv --source "$ROOT/migrations" >/dev/null
  say "test server ready: $TEST_NAME on 127.0.0.1:$SERVER_PORT"
  export "${DB_ENV}_DB_CONTAINER=$TEST_NAME" DATABASE_URL="$TEST_URL"
  if [ $share -eq 1 ]; then
    export "${DB_ENV}_DB_SHARE=1" "${DB_ENV}_TEST_DATABASE_URL=$TEST_URL"
  else
    export "${DB_ENV}_DB_SHARE=" "${DB_ENV}_TEST_DATABASE_URL="
  fi
  status=0
  # The command runs in the background and the script waits for it, so a
  # signal to this script runs the clean-up at once; bash runs a trap only
  # after a foreground command ends. Job control (set -m) gives the command
  # its own process group, so the clean-up can stop cargo and the test
  # binaries too, and the command does not ignore SIGINT.
  set -m
  "$@" <&0 &
  CHILD=$!
  set +m
  wait "$CHILD" || status=$?
  CHILD=""
  exit $status
}

cmd_gc() {
  local name state role worktree owner port run repo path major marker
  require_container
  clean_up_servers
  while IFS=$'\t' read -r name state role worktree owner port run repo path major; do
    [ "$role" != dev ] && [ "$worktree" = "$WORKTREE_ID" ] && [ -e "$COMMON/kept/$name" ] || continue
    say "removing the kept $role server $name"
    remove_server "$name"
    container volume delete "$name" >/dev/null 2>&1 || true
    rm -f "$COMMON/kept/$name"
  done < <(list_servers)
  # Markers of servers that no longer exist.
  for marker in "$COMMON"/kept/*; do
    [ -e "$marker" ] || continue
    name="$(basename "$marker")"
    [ -n "$(server_line "$name")" ] || container volume inspect "$name" >/dev/null 2>&1 ||
      rm -f "$marker"
  done
}

# `cargo sqlx prepare` of this worktree, one at a time: prepare rewrites
# .sqlx, both modes write target/sqlx-tmp, and check mode clears and writes
# target/sqlx-prepare-check. Runs inside with-test-db, which sets
# DATABASE_URL.
cmd_sqlx() { # $1 = prepare | check
  local check=()
  case "${1:-}" in
    prepare) ;;
    check) check=(--check) ;;
    *) die "usage: $0 sqlx prepare|check" ;;
  esac
  [ -n "$(env_value DB_CONTAINER)" ] || die "run sqlx through with-test-db"
  mkdir -p "$STATE"
  lock "$STATE/sqlx.lock" "the SQLx metadata lock of this worktree"
  trap 'unlock "$STATE/sqlx.lock"' EXIT
  SQLX_OFFLINE=false cargo sqlx prepare --no-dotenv --workspace ${check[@]+"${check[@]}"} -- --all-targets --locked
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

# Other scripts (for example a rehearsal) source this file for its functions.
# Keep the function names apart from common names such as `cleanup`, which a
# sourcing script can define for itself.
[ "${BASH_SOURCE[0]}" = "$0" ] || return 0

command="${1:-}"
[ $# -gt 0 ] && shift
case "$command" in
  setup) cmd_setup ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  info) cmd_info ;;
  psql) cmd_psql "$@" ;;
  with-dev-db) cmd_with_dev_db "$@" ;;
  with-test-db) cmd_with_test_db "$@" ;;
  dump) cmd_dump "${1:-}" ;;
  restore) cmd_restore "${1:-}" ;;
  clone) cmd_clone "${1:-}" ;;
  reset) cmd_reset ;;
  gc) cmd_gc ;;
  sqlx) cmd_sqlx "$@" ;;
  *)
    # The commands of scripts/db-project.sh (DB_PROJECT_COMMANDS).
    case " ${DB_PROJECT_COMMANDS:-} " in
      *" $command "*)
        project_command "$command" "$@"
        exit $?
        ;;
    esac
    sed -n '2,/^set -euo/p' "$0" | sed '$d' >&2
    exit 2
    ;;
esac

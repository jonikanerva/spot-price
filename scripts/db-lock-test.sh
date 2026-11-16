#!/usr/bin/env bash
# Test of the locks of scripts/db.sh: two processes never hold one lock, one
# test server slot, or the dev lock of the clean-up, also when both find the
# same stale lock.
#
# This file is the same in caltodo, mastofeed, and spot-price. It uses only a
# temporary directory: no container and no database.
#
# Process B finds the stale lock and pauses at its first or second stale
# verdict (the second one is the check again before the removal). Process A
# then finds the same stale lock. B continues only after A holds the lock,
# or after 3 s. A correct lock never lets both hold it.
#
# A last case changes the owner during the stale check: B reads the owner X
# of a live lock, X unlocks and ends, and C takes the lock before B checks X.
# B must not remove the lock of C.
#
# The ps case makes ps fail. A live owner must stay alive, and its lock must
# stay. With a working ps, the owner is gone only after it ends.
set -euo pipefail

SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
DB_ENV="$(sed -n 's/^DB_ENV=//p' "$SCRIPTS/db-project.sh")"
image_var="${DB_ENV}_PG_IMAGE"
export "$image_var=${!image_var:-postgres:18}"

wait_for() { # $1 = file, $2 = limit in 0.1 s steps
  local i
  for i in $(seq 1 "$2"); do
    [ -e "$1" ] && return 0
    sleep 0.1
  done
  return 1
}

worker() { # $1 = directory, $2 = lock or slot, $3 = role a or b, $4 = pause verdict
  # Globals: the functions of db.sh have locals named dir and role, and bash
  # scopes variables dynamically.
  LOCK_TEST_DIR="$1"
  LOCK_TEST_KIND="$2"
  LOCK_TEST_ROLE="$3"
  LOCK_TEST_PAUSE="${4:-1}"
  # shellcheck source=scripts/db.sh
  . "$SCRIPTS/db.sh"
  COMMON="$LOCK_TEST_DIR"
  MAX_TEST=1
  eval "orig_$(declare -f dir_stale)"
  dir_stale() {
    orig_dir_stale "$1" || return 1
    [ "$LOCK_TEST_ROLE" = b ] || return 0
    # A subshell keeps its own count: count in a file.
    echo x >>"$LOCK_TEST_DIR/b-verdicts"
    LOCK_TEST_VERDICTS="$(wc -l <"$LOCK_TEST_DIR/b-verdicts" | tr -d ' ')"
    if [ "$LOCK_TEST_VERDICTS" = "$LOCK_TEST_PAUSE" ]; then
      : >"$LOCK_TEST_DIR/b-paused"
      wait_for "$LOCK_TEST_DIR/a-holds" 30 || true
    fi
    return 0
  }
  if [ "$LOCK_TEST_KIND" = dev ]; then
    # remove_stopped_dev holds the dev lock while it reads the container
    # state. The stub checks the holder there and finds no container.
    server_line() {
      [ "$LOCK_TEST_ROLE" = a ] && : >"$LOCK_TEST_DIR/a-holds"
      if ! mkdir "$LOCK_TEST_DIR/holder" 2>/dev/null; then
        echo "$LOCK_TEST_ROLE" >>"$LOCK_TEST_DIR/violations"
      fi
      sleep 1
      rmdir "$LOCK_TEST_DIR/holder" 2>/dev/null || true
    }
    remove_stopped_dev the-dev-server "$LOCK_TEST_DIR/worktree"
    return 0
  fi
  if [ "$LOCK_TEST_KIND" = lock ]; then
    lock "$LOCK_TEST_DIR/the.lock" "the test lock"
  else
    acquire_slot
  fi
  [ "$LOCK_TEST_ROLE" = a ] && : >"$LOCK_TEST_DIR/a-holds"
  if ! mkdir "$LOCK_TEST_DIR/holder" 2>/dev/null; then
    echo "$LOCK_TEST_ROLE" >>"$LOCK_TEST_DIR/violations"
  fi
  sleep 1
  rmdir "$LOCK_TEST_DIR/holder" 2>/dev/null || true
  if [ "$LOCK_TEST_KIND" = lock ]; then
    unlock "$LOCK_TEST_DIR/the.lock"
  else
    unlock "$SLOT"
  fi
}

owner_change() { # $1 = test directory
  local x b
  # A global: the functions of db.sh can have a local named dir.
  OWNER_TEST_DIR="$1"
  # shellcheck source=scripts/db.sh
  . "$SCRIPTS/db.sh"
  sleep 30 &
  x=$!
  mkdir "$OWNER_TEST_DIR/the.lock"
  owner_of "$x" >"$OWNER_TEST_DIR/the.lock/owner"
  (
    eval "orig_$(declare -f owner_alive)"
    owner_alive() {
      : >"$OWNER_TEST_DIR/b-read"
      wait_for "$OWNER_TEST_DIR/c-holds" 50 || true
      orig_owner_alive "$1"
    }
    remove_if_stale "$OWNER_TEST_DIR/the.lock"
  ) &
  b=$!
  wait_for "$OWNER_TEST_DIR/b-read" 50 || return 1
  rm -r -f "$OWNER_TEST_DIR/the.lock"
  kill "$x"
  wait "$x" 2>/dev/null || true
  mkdir "$OWNER_TEST_DIR/the.lock"
  owner_of $$ >"$OWNER_TEST_DIR/the.lock/owner"
  : >"$OWNER_TEST_DIR/c-holds"
  wait "$b" || true
  [ -d "$OWNER_TEST_DIR/the.lock" ]
}

ps_failure() { # $1 = test directory
  local x token fake failed=0
  # shellcheck source=scripts/db.sh
  . "$SCRIPTS/db.sh"
  sleep 30 &
  x=$!
  token="$(owner_of "$x")"
  mkdir "$1/the.lock"
  echo "$token" >"$1/the.lock/owner"
  fake="$1/bin"
  mkdir "$fake"
  # ps cannot start, and ps reports an error.
  printf '#!/bin/sh\nexit 126\n' >"$fake/ps"
  chmod +x "$fake/ps"
  PATH="$fake:$PATH" owner_alive "$token" || failed=1
  PATH="$fake:$PATH" remove_if_stale "$1/the.lock"
  [ -d "$1/the.lock" ] || failed=1
  printf '#!/bin/sh\necho "ps: an error" >&2\nexit 1\n' >"$fake/ps"
  PATH="$fake:$PATH" owner_alive "$token" || failed=1
  # With a working ps: the live owner stays, and the gone owner is gone.
  owner_alive "$token" || failed=1
  kill "$x"
  wait "$x" 2>/dev/null || true
  ! owner_alive "$token" || failed=1
  dir_stale "$1/the.lock" || failed=1
  return $failed
}

if [ "${1:-}" = worker ]; then
  shift
  worker "$@"
  exit 0
fi
if [ "${1:-}" = owner-change ]; then
  owner_change "$2"
  exit $?
fi
if [ "${1:-}" = ps-failure ]; then
  ps_failure "$2"
  exit $?
fi

base="$(mktemp -d "${TMPDIR:-/tmp}/db-lock-test.XXXXXX")"
trap 'rm -r -f "$base"' EXIT
failed=0
for case in lock:1 slot:1 dev:1 lock:2 slot:2 dev:2; do
  kind="${case%:*}"
  pause="${case#*:}"
  dir="$base/$kind-$pause"
  mkdir -p "$dir"
  if [ "$kind" = lock ]; then
    stale="$dir/the.lock"
  elif [ "$kind" = dev ]; then
    stale="$dir/worktree/.tmp/db/dev.lock"
  else
    stale="$dir/slots/1"
  fi
  mkdir -p "$stale"
  # PID 1 runs, but with another start time: the owner is gone.
  echo "1@stale" >"$stale/owner"
  "$0" worker "$dir" "$kind" b "$pause" &
  b=$!
  label="db-lock-test: $kind, pause at verdict $pause"
  if ! wait_for "$dir/b-paused" 100; then
    echo "$label: process B did not reach the pause" >&2
    failed=1
  fi
  "$0" worker "$dir" "$kind" a &
  a=$!
  wait "$a" "$b"
  if [ -s "$dir/violations" ]; then
    echo "$label: two processes held the lock at the same time" >&2
    failed=1
  elif [ -e "$dir/b-paused" ]; then
    echo "$label: ok"
  fi
done
mkdir -p "$base/owner-change"
if "$0" owner-change "$base/owner-change"; then
  echo "db-lock-test: owner change during the check: ok"
else
  echo "db-lock-test: owner change during the check: the check removed a live lock" >&2
  failed=1
fi
mkdir -p "$base/ps-failure"
if "$0" ps-failure "$base/ps-failure"; then
  echo "db-lock-test: an error of ps keeps a live owner: ok"
else
  echo "db-lock-test: an error of ps made a live owner look gone" >&2
  failed=1
fi
exit $failed

#!/usr/bin/env bash
# Test of the locks of scripts/db.sh: two processes never hold one lock or
# one test server slot, also when both find the same stale lock.
#
# This file is the same in caltodo, mastofeed, and spot-price. It uses only a
# temporary directory: no container and no database.
#
# Process B finds the stale lock and pauses at its first or second stale
# verdict (the second one is the check again before the removal). Process A
# then finds the same stale lock. B continues only after A holds the lock,
# or after 3 s. A correct lock never lets both hold it.
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

if [ "${1:-}" = worker ]; then
  shift
  worker "$@"
  exit 0
fi

base="$(mktemp -d "${TMPDIR:-/tmp}/db-lock-test.XXXXXX")"
trap 'rm -r -f "$base"' EXIT
failed=0
for case in lock:1 slot:1 lock:2 slot:2; do
  kind="${case%:*}"
  pause="${case#*:}"
  dir="$base/$kind-$pause"
  mkdir -p "$dir"
  if [ "$kind" = lock ]; then
    stale="$dir/the.lock"
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
exit $failed

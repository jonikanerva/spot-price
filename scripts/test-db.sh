#!/usr/bin/env bash
# The isolated PostgreSQL server of the tests and the SQLx query checks
# (STACK.md §3). It runs the pinned PostgreSQL 17 from mise, listens only on
# 127.0.0.1:55436, trusts local connections, and keeps its data in
# .tmp/postgres. It never touches another server.
#
# Usage: scripts/test-db.sh start|stop|reset
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATA="$ROOT/.tmp/postgres"
PORT=55436
URL="postgres://postgres@127.0.0.1:$PORT/spot_price_dev"
BIN="$(mise where conda:postgresql)/bin"

start() {
  if [ ! -f "$DATA/PG_VERSION" ]; then
    mkdir -p "$DATA"
    "$BIN/initdb" --pgdata="$DATA" --username=postgres --auth=trust --encoding=UTF8 --no-locale >/dev/null
  fi
  if ! "$BIN/pg_ctl" --pgdata="$DATA" status >/dev/null 2>&1; then
    "$BIN/pg_ctl" --pgdata="$DATA" --log="$DATA/server.log" --wait \
      -o "-p $PORT -c unix_socket_directories= -c listen_addresses=127.0.0.1 -c fsync=off" start >/dev/null
  fi
  "$BIN/psql" --host=127.0.0.1 --port=$PORT --username=postgres --quiet --tuples-only \
    --command="SELECT 1 FROM pg_database WHERE datname = 'spot_price_dev'" | grep -q 1 ||
    "$BIN/createdb" --host=127.0.0.1 --port=$PORT --username=postgres spot_price_dev
  # --no-dotenv: never read a .env file.
  DATABASE_URL="$URL" sqlx migrate run --no-dotenv --source "$ROOT/migrations" >/dev/null
  echo "test database ready: $URL"
}

stop() {
  if [ -f "$DATA/PG_VERSION" ]; then
    "$BIN/pg_ctl" --pgdata="$DATA" --mode=fast stop >/dev/null 2>&1 || true
  fi
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  reset) stop; rm -rf "$DATA"; start ;;
  *) echo "usage: $0 start|stop|reset" >&2; exit 2 ;;
esac

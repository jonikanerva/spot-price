# STACK.md — Spot-price: Rust / Axum service with PostgreSQL

> One Rust service serves the spot-price REST API, a small server-rendered
> setup page, and the OpenAPI document. It runs the scheduled Nord Pool,
> Fingrid, and OpenWeatherMap fetch jobs in the same process. Axum, Tokio,
> and Tower provide HTTP and concurrency. SQLx gives checked SQL on
> PostgreSQL. Jiff represents time. There is no browser application build.

This file adapts the agent-setup profile `stacks/STACK-RUST.md` (selection of
13 October 2026) to this repository. Use it with `DOCTRINE.md` (P1–P9) and
`CLAUDE.md`. MUST, MUST NOT, SHOULD, and MAY have the meanings in the
doctrine. A rule of the profile that this file does not adopt is listed in
§0 "Not applicable" with its reason, or in §13 as an exception. This file
grants no authority beyond `CLAUDE.md`.

---

## 0. Project shape

- **Project risk rationale:** one household's home automations depend on
  this API to schedule flexible loads (sauna, EV, heating). A wrong or
  missing price shifts load to an expensive hour or skips it; the cost is
  money and comfort, not safety. Personal data is limited to one account,
  contract settings, and API keys (`VISION.md → Persistence and Privacy
Posture`). Price and grid data are public and can be fetched again. Most
  failures are recoverable by a revert PR or a fix PR within minutes. The
  database has no backup (§5), so a destructive migration or a database
  loss is the main irreversible risk.
- **Change risk:** assess each change against that rationale. Changes to
  price math, the cheapest-window answer, the v1 wire format, sign-in,
  sessions, API keys, migrations, retention, or the release configuration
  are material. A risk label does not waive required checks or review.
- **Shape:** one Rust service in a three-crate workspace. The setup page is
  one HTML document with one inline script, rendered once at start-up.
- **Critical execution paths:** (1) API request: key check, rate limit,
  query validation, store read, domain calculation, JSON writer; (2) the
  day-ahead price fetch job. Blocking work (scrypt, the forecast fit) runs on
  the blocking pool, not on async request tasks.
- **Applicable states:** API answers are a typed success or a typed error.
  When Nord Pool has not published, the answer is `available: false` or
  `404`, never an estimate. The setup page handles awaiting first data,
  success, empty, signed out, and error.
- **Reason for Rust:** owner decision (2026-10-14) to align with the owner's
  other Rust services on Railway (mastofeed, commute, caltodo, psstore,
  weather) and to reduce memory use.

### Not applicable (with reasons)

| Profile part                                                                     | Reason                                                                                                                                                            |
| -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Frontend SPA: TypeScript 7, React, Vite, TanStack, Oxlint, Vitest (§1, §2, §2.4) | The UI is a setup page only (`VISION.md`). `crates/server/src/http/ui/` holds the HTML and the inline script of the TypeScript service, except the chart drawing. |
| Hey API client and Zod validators (§2.1 steps 3–5)                               | No first-party client consumes the API. Home Assistant and scripts are the clients. `contract/openapi.json` is committed and drift-checked.                       |
| RFC 9457 Problem Details (§2.2)                                                  | The v1 contract keeps `{"error": "..."}` (docs/adr/0003). A change needs `/api/v2`.                                                                               |
| OIDC (§2.5)                                                                      | The product uses local username and password accounts (docs/adr/0002).                                                                                            |
| Exact decimals (§7)                                                              | Prices are `DOUBLE PRECISION` and binary floats with JavaScript rounding (docs/adr/0003).                                                                         |
| axe accessibility checks (§11)                                                   | Not adopted in the port. Only the chart drawing differs from the TypeScript page. The keyboard and contrast thresholds of §11 stay review checks.                 |
| proptest, uuid, rust_decimal, Tailwind                                           | No current need. Golden files and seeded sweeps cover the rounding rules.                                                                                         |

### Repository layout and boundaries

```text
crates/
  domain/src/          # areas, price math, local-day rules, query rules, JavaScript number rules, forecast
  domain/tests/        # golden tests against TypeScript output
  server/src/          # config, store, upstream clients, jobs, HTTP, auth, logging, lifecycle
  server/tests/        # integration tests, HTTP golden replay, schema snapshot
  server/sqlx.toml     # SQLx macro type override (TIMESTAMPTZ -> jiff_sqlx::Timestamp)
  server/build.rs      # rebuild the server when migrations/ changes
  tools/               # offline forecast tools (backtest, regenerate-bands, revision-magnitude)
migrations/            # SQLx migrations
.sqlx/                 # committed SQLx query metadata
contract/openapi.json  # exported OpenAPI 3.1 document
e2e/                   # Playwright journeys against the Rust binary
scripts/               # db.sh, db-lock-test.sh, smoke-local.sh, schema-snapshot.sql, golden generators (history only)
docs/adr/              # decisions
Cargo.toml  Cargo.lock  rust-toolchain.toml  clippy.toml  deny.toml  mise.toml
railpack.json  .gitleaks.toml  package.json  pnpm-lock.yaml
STACK.md  VISION.md  DOCTRINE.md  CLAUDE.md
```

- `spot-price-domain` depends only on `jiff`, `serde`, and `thiserror`. It
  MUST NOT depend on Axum, Tokio, SQLx, reqwest, or the server crate. It
  reads no clock, environment, or file; callers pass `now` and the data.
- `spot-price-server` owns all effects: configuration (`config.rs`), the
  store (`store/`), the upstream clients (`upstream/`), the jobs (`jobs/`),
  HTTP (`http/`), sign-in (`auth/`), logging (`observability.rs`), and the
  process lifecycle (`lifecycle.rs`, `main.rs`).
- `spot-price-tools` is an offline binary. The release build compiles only
  `spot-price-server`, so the tools never ship. The tools depend on the
  domain crate, not on the server crate.
- Cargo enforces the crate dependencies. Purity inside the domain crate is a
  review check.

---

## 1. Language & Runtime

- **Language:** Rust 1.100.0, edition 2024, pinned in `rust-toolchain.toml`
  with rustfmt and Clippy. `rust-version = "1.100"` in the workspace; the
  server crate repeats it (§15, Railpack).
- **Runtime:** a native Linux binary in the Railpack runtime image. A local
  macOS build does not validate the Linux artifact.
- **Package manager:** Cargo with workspace dependency inheritance, committed
  `Cargo.lock`, `--locked` on every task.
- **Strictness:** the workspace lints of the profile (`unsafe_code =
"forbid"`, Clippy pedantic, and the deny list) plus
  `missing_debug_implementations`. `clippy.toml` allows unwrap, expect,
  panic, and print in tests.
- **Node.js:** Node 26 and pnpm 12 run only Playwright and Prettier. They
  are not in the production image.

### Provisioning

Install mise and rustup. Then run in the repository root:

```bash
mise trust && mise run setup
```

Install the Apple `container` tool 1.0.0 or later and run `container system
start` once. The Mac has no PostgreSQL install: every local server runs in an
Apple container (§3). `rust-toolchain.toml` owns the Rust version.
`SPOT_PRICE_PG_IMAGE` in `mise.toml` owns the PostgreSQL image. `mise.toml`
owns the other tools: Node, pnpm, sqlx-cli 0.9.0,
cargo-deny, cargo-nextest, cargo-shear, and Gitleaks.

---

## 2. Frameworks

| Concern             | Technology                                   | Role                                           |
| ------------------- | -------------------------------------------- | ---------------------------------------------- |
| HTTP                | Axum 0.8, tower-http 0.7                     | Routes, extraction, panic containment          |
| Concurrency         | Tokio 1 (multi-thread, 2 workers)            | Async I/O, jobs, signals, blocking pool        |
| Errors              | `Result`, enums, thiserror                   | Fixed-text error kinds per adapter             |
| Wire representation | serde, serde_json, a first-party JSON writer | JavaScript number format (docs/adr/0003)       |
| HTTP contract       | utoipa 6, utoipa-axum 0.3                    | OpenAPI 3.1 document of the API routes         |
| Time                | Jiff 0.2 (bundled tzdb), jiff-sqlx 0.2       | Instants, local days, DST                      |
| Schedule            | croner 4 (`jiff` feature)                    | Job schedules in UTC and Europe/Oslo           |
| Persistence         | PostgreSQL 18, SQLx 0.9                      | Checked query macros, migrations, transactions |
| Outbound HTTP       | reqwest 0.13 (rustls, gzip)                  | Nord Pool, Fingrid, OpenWeatherMap             |
| Passwords           | scrypt, unicode-normalization, subtle        | Better Auth-compatible hashes (docs/adr/0002)  |
| Logging             | tracing, tracing-subscriber                  | One JSON line per event with a field allowlist |
| Tests               | nextest, `#[sqlx::test]`, Playwright         | Unit, golden, integration, browser journeys    |
| Formatting and lint | rustfmt, Clippy, Prettier                    | Named checks through mise                      |

### 2.1 Public contract

- `/api/v1/*`, `/api/public/spot`, `/api/session*`, `/api/keys*`, and
  `/health` keep the TypeScript wire format byte for byte
  (docs/adr/0003). `crates/server/tests/golden/http/` holds 91 recorded cases
  of the TypeScript service; `golden_http.rs` replays them. The test lists
  each intentional difference and fails when one stops occurring. The
  `forecast-values` kind accepts a forecast body that equals the recorded
  body byte for byte after it masks the `estimated*CentsKwh` values; each
  masked value must be a finite JSON number. The forecast training rule
  (`LatestPublishedRule::Before`) and the within-day shape (the four-week
  weekday mean, §9) differ from the TypeScript service.
  The `generated-at` kind sets a differing `generatedAt` of a forecast body
  to the recorded value before the other body checks, so one step can list
  both `generated-at` and `forecast-values`.
  The `tomorrow-available` kind sets a `"tomorrowAvailable":false` of a body
  to `true` when the recorded body has `true`, before the other body checks.
  The Rust service is true only when the store has every quarter of the next
  local day; the TypeScript service was true for any stored row of that day.
  The `tomorrow-unavailable` kind accepts a `/api/v1/price/tomorrow` body
  that is exactly the unpublished list where the recorded body is an
  available list with prices. The Rust route is available only for a
  complete next local day.
- `contract/openapi.json` is the export of `spot-price export-openapi`. The
  `contract:check` task fails on drift. `contract.rs` compares it with the
  TypeScript document field by field. The one intentional difference is the
  `description` of `generatedAt` in the forecast response schema. The golden
  kind `generated-at-description` accepts only that difference in the
  recorded OpenAPI body.
- `/api/docs` serves the Scalar reference page of the TypeScript service. It
  loads Scalar from the jsDelivr CDN in the browser.
- A breaking change needs `/api/v2` (`VISION.md`).

### 2.2 Errors and failure containment

- API errors are `{"error": "..."}` with the texts of the TypeScript service,
  including the zod validation texts joined with `; `.
- An unexpected failure gives 500 `Internal Server Error` as `text/plain`,
  like the TypeScript service. `CatchPanicLayer` turns a handler panic into
  the same answer.
- A stored settings row that breaks the domain rules (an unsupported time
  zone, an hour outside 0–23) gives 500 and logs `invalid_settings` with the
  user id only.
- Adapters map every failure to a fixed kind. Transport error text never
  leaves the adapter, because it can contain a URL with a key.
- Upstream data: an invalid Nord Pool entry is left out; an invalid Fingrid
  record degrades the whole response; the OpenWeatherMap hourly and daily
  blocks degrade separately (§9).

### 2.3 Outbound sources

One shared reqwest client: `User-Agent: spot-price/<version>`, redirects off,
connect timeout 10 s, a 30 s deadline per attempt that covers the body, and a
16 MiB body limit. Transport errors use `without_url`.

- **Nord Pool:** `GET dataportal-api.nordpoolgroup.com/api/DayAheadPrices`
  for all 21 areas in one call; 3 attempts, 7 s apart.
- **Fingrid:** `GET data.fingrid.fi/api/data`, header `x-api-key`; no
  retry, never throws. Each run sends two requests, 2 s apart, because
  Fingrid allows one request every 2 s per key. The first request gets
  datasets 245, 75, 165, 124 for the served forecast. The second request
  gets datasets 166 (consumption forecast, 72 h) and 248 (solar forecast,
  72 h) for `[issued_at − 6 h, now + 73 h)`. A failure of one request does
  not affect the other request.
- **OpenWeatherMap:** One Call 3.0 for two fixed FI points; the key is in the
  query, so errors never include the URL.

### 2.4 Sign-in and sessions

docs/adr/0002 records the design. In short: Better Auth scrypt hashes in
`account.password`; sign-up writes Better Auth rows in one transaction;
sessions in `app_sessions` (SHA-256 of a 32-byte random token, 7-day expiry,
renewed after one day, no IP or user agent); cookie `spot_session`
(`__Host-spot_session` with `Secure` on HTTPS), `HttpOnly`, `SameSite=Lax`;
a foreign `Origin` on a state-changing session route gives 403. scrypt runs
on the blocking pool behind a semaphore of two.

---

## 3. Build & verify commands

| Variable      | Command           |
| ------------- | ----------------- |
| `$FORMAT_CMD` | `mise run format` |
| `$LINT_CMD`   | `mise run lint`   |
| `$BUILD_CMD`  | `mise run build`  |
| `$TEST_CMD`   | `mise run test`   |
| `$VERIFY_CMD` | `mise run verify` |

- **format:** `cargo fmt --all` and Prettier. `format-check` runs both in
  check mode.
- **lint:** `cargo clippy --workspace --all-targets --locked -- -D warnings`
  with `SQLX_OFFLINE=true`, and `tsc` on the e2e TypeScript (`e2e/tsconfig.json`).
- **build:** `cargo build --release --locked --package spot-price-server`
  with `SQLX_OFFLINE=true`.
- **test:** runs `scripts/db-lock-test.sh` (the lock test of
  `scripts/db.sh`; a temporary directory only, no container). Then it starts
  a new test server and runs `cargo nextest run --workspace --locked` and
  `cargo test --doc --workspace --locked`.
- **e2e:** the Playwright journeys in `e2e/` against a copy of the release
  binary, on a new test server.
- **verify:** format-check, lint, `sqlx:check`, `contract:check`,
  `scan:secrets`, `scan:deps`, build, test, and e2e, in this order. It stops
  at the first failed step. The Playwright journeys are part of `verify`.
  All steps of one `verify` run share one test server.
- **dev:** starts the dev server of the worktree, migrates it, and runs the
  service with `.env.local`. At exit it stops the dev server.
- **Other tasks:** `sqlx:prepare` (regenerate `.sqlx` after a query or
  migration change) and `setup`. The `db:` tasks are optional tools:
  `db:start` / `db:stop` (keep the dev server running / stop it), `db:info`,
  `db:psql`, `db:dump`, `db:restore`, `db:clone`, `db:reset` (dev data), and
  `db:gc` (the clean-up now, and the kept test servers of the worktree).
- **Dependency freshness check** (`DOCTRINE.md` P7, read-only): run
  `cargo update --dry-run --verbose`, `pnpm outdated --recursive`, and
  `mise outdated --bump`. Each crate pin in `Cargo.toml` is exact, so also
  compare each direct crate with its latest stable release on crates.io.
  Compare `rust-toolchain.toml` with the latest stable Rust release. Compare
  `SPOT_PRICE_PG_IMAGE` with the latest `postgres:18.x` tag on Docker Hub.
  Compare
  each infrastructure version in §15 with its latest stable release. Record
  the release date of each candidate version (§15, Dependencies). The checks
  need network access to crates.io, npm, GitHub, and the mise version
  service. A sandbox that blocks them gives no result; that is a gap, not a
  pass.

**Local PostgreSQL.** `scripts/db.sh` runs every local server in an Apple
container of `SPOT_PRICE_PG_IMAGE`. The tasks start and remove the servers
themselves, so a developer or an agent runs only `dev`, `test`, `e2e`,
`sqlx:*`, or `verify`. Agents and people never start, stop, or remove the
containers or volumes by hand.

`scripts/db.sh` and `scripts/db-lock-test.sh` are the same files in caltodo,
mastofeed, and spot-price; keep them byte-identical. `scripts/db-project.sh` holds the settings and the
commands of this project: the names, the port range, the service address of
`dev`, and `e2e`.

- **Dev server:** one per worktree, `spot-price-dev-<id>`, with the database
  `spot_price` in the volume `spot-price-dev-<id>-pg<major>`. `<id>` is a hash
  of the physical checkout path. The server uses the durable PostgreSQL
  defaults. A command that needs it (`dev`, `db:psql`, `db:dump`,
  `db:restore`, `db:clone`, `db:reset`) starts it, and the last such command
  of the worktree stops it at exit. `db:start` keeps it running until
  `db:stop`. The data stays in the volume. A stop removes the container, because a stopped container keeps a copy of the image (about 750 MB). The data stays in the volume, and the next start creates the container again on a new port. The clean-up also removes stopped dev containers, for example of an earlier version of the script.
  `db:clone` dumps the source worktree through the same rules: it starts the
  source server when needed and stops it again. A volume of another major version
  stays, and the new major version gets a new volume; move data with
  `db:dump` and `db:restore`. A volume without the `-pg<major>` suffix comes
  from the first version of the script and holds major version 18. `dev`
  sets `DATABASE_URL`, `PORT`, and `BETTER_AUTH_URL`; the service port is
  kept per worktree in `.tmp/db/app-port`. A `PORT` and `BETTER_AUTH_URL` of
  `.env.local` win. A `DATABASE_URL` of `.env.local` never selects the
  database; `SPOT_PRICE_DATABASE_URL` does.
- **Dev data:** `db:restore`, `db:clone`, and `db:reset` replace the dev
  database. They first write a non-empty database to `.tmp/db/backups/`. The
  next `dev` migrates the result.
- **Test server:** one per independent run of `test`, `e2e`, `sqlx:check`,
  `sqlx:prepare`, or `verify`, `spot-price-test-<id>-<random>`. It starts
  empty, is migrated, keeps its data in tmpfs, and runs with `fsync`,
  `synchronous_commit`, and `full_page_writes` off. These settings never
  reach a dev server. The test tasks set `DATABASE_URL` to that server and
  unset the `PG*` variables, so a `DATABASE_URL` of the shell or `.env.local`
  cannot reach a test. `#[sqlx::test]` creates and drops one database per
  test. A nested task reuses the server of the enclosing run only when that
  run passes `--share` (`verify` does) and the server still runs.
- **Ports and access:** a random host port in 40000–48999 on `127.0.0.1`
  only. caltodo uses 20000–29999 and mastofeed 30000–39999; all stay below
  the macOS ephemeral range. A port in use makes `container run` fail, and
  the script retries with another port. Each server has a random password
  (dev: `.tmp/db/dev-password`), because other containers of the Mac can
  reach the container network. No TLS on local servers.
- **Other checkouts:** `SPOT_PRICE_DB_ROOT=<checkout>` makes a `db:*` task
  act on the dev server of that checkout. A task that mise finds in the
  `mise.toml` of an outer checkout stops instead of using the servers of
  that checkout.
- **Limits:** 2 CPUs and 1 GiB per server (`SPOT_PRICE_DB_CPUS`,
  `SPOT_PRICE_DB_MEMORY`). At most 4 test servers run at the same time over
  all worktrees of this repository (`SPOT_PRICE_DB_MAX_TEST`); a run waits
  for a free slot, at most 2 hours. The locks wait at most 2 hours too.
  A lock or a slot is a directory with an owner file. A process removes a
  stale one only under a kernel lock (`flock`) on its `.guard` file, after a
  second check. Two processes never hold one lock when every worktree runs
  this version of `scripts/db.sh`. An owner counts as gone only when `ps` runs
  and finds no process with its PID and start time. An error of `ps` keeps the
  lock, the slot, and the test server of the owner.
- **Clean-up:** a run removes its own test server on exit, also on Ctrl-C and
  on a SIGTERM to the script: the command runs in its own process group, and
  the clean-up stops it first. Every command that starts a server, and
  `db:gc`, first removes:
  - the stopped dev containers that no command is creating (under the dev lock of their worktree; their volumes stay);
  - the test servers of this repository whose owner process is gone; a kept
    test server stays;
  - the dev server and the dev volume of a worktree whose directory is gone,
    when git no longer lists the worktree and the parent directory still
    exists. This deletes the dev data of that worktree. Move a worktree only
    with its data: `db:dump` before, `db:restore` after.
- **Keep a failed run:** `SPOT_PRICE_DB_KEEP=failed` (or `always`) keeps the
  server of a failed run and writes its log to `.tmp/db/logs`; `db:gc`
  removes it.
- **E2E files:** each run uses `.tmp/e2e/<test server>/` for the binary copy
  and the Playwright output, and a free service port. A failed run keeps the
  directory. The release build and the copy hold a worktree lock. A manual
  `mise run build` does not take the lock, so do not run it beside `e2e` in
  the same worktree.
- **SQLx metadata:** `sqlx:prepare` and `sqlx:check` hold a worktree lock,
  because prepare rewrites `.sqlx` and both use the SQLx work files in
  `target/`.
- **Agent sandbox:** `.claude/settings.json` lists these tasks in
  `sandbox.excludedCommands`, because the `container` tool needs XPC. The
  entries match only a plain call such as `mise run verify`. A call with
  `cd`, a pipe, or a redirect stays in the sandbox and fails.

**Local database measurements** (2026-11-15, Apple silicon Mac, container
1.0.0, image `postgres:18.7`; other agents loaded the Mac):

| Step                                                      | Result                                                                     |
| --------------------------------------------------------- | -------------------------------------------------------------------------- |
| Image pull, `--platform linux/arm64`                      | About 8 s. A pull without `--platform` unpacks every platform: about 6 min |
| Test server: start, migrate, and remove (`-- true`)       | 2.3–5.9 s. The former local `initdb` server took 0.9–1.1 s                 |
| Dev server: first start / restart / start after `kill`    | 12 s / 4–6 s / 4 s                                                         |
| `cargo nextest -p spot-price-server` (203 database tests) | 24–29 s in a container, 24–31 s on the former local server                 |
| `mise run verify`                                         | 671 s, load average above 100                                              |
| Memory                                                    | Test server about 525 MiB of 1 GiB during `test`; idle dev server 233 MiB  |

The readiness check sends a PostgreSQL SSLRequest to the published port; an
open TCP port is not enough. When the Mac changes the bridge of the container
network, the published port of an older container accepts connections but
never answers. `db:start` and `dev` then create the dev container again; the data
stays in the volume.

The container adds a few seconds of start-up per run. The database work
itself has the same speed. Limitations: the `container` service must run;
a sandbox that blocks XPC stops the database tasks; the first start of a
worktree creates its volume.

**No `.env` file.** The SQLx query macros parse every `.env` file in the
crate directory and its parent directories at compile time. Keep local
settings in `.env.local`. sqlx-cli runs with `--no-dotenv`.

**Sandboxes.** The `container` tool talks to its system service over XPC,
and the tests bind `127.0.0.1` ports. A sandbox that blocks either makes the
database tasks fail with "the container system service does not answer";
that is a missing environment, not a pass.

---

## 4. Performance budgets

| Budget      | Adopted value and conditions                                                                      |
| ----------- | ------------------------------------------------------------------------------------------------- |
| API request | p99 100 ms, p50 30 ms per route, excluding upstream calls (unchanged from the TypeScript service) |
| Cold start  | Ready within 2 s, excluding migrations                                                            |
| Memory      | Container limit 375 MB (Railway setting). scrypt uses at most 64 MiB (two hashes at 32 MiB).      |

The PR of the port records the measurements. Do not raise a budget to fit a
result.

---

## 5. Persistence shape

- **Storage:** Railway PostgreSQL 18 (image `postgres-ssl:18`). The schema
  is the schema of the TypeScript migrations `001`–`007` plus
  `app_sessions`.
- **Migrations:** numbered SQLx files in `migrations/`. `0001_baseline.sql`
  is idempotent and changes nothing on the production schema; the old ledger
  `_migrations` stays. `0002_app_sessions.sql` adds the session table.
  `0003_drop_redundant_indexes.sql` drops three indexes that duplicate other
  indexes, with a `lock_timeout` of 5 s; after it, the production catalog
  differs from `fixtures/schema-snapshot.txt` by exactly these three
  indexes. The migration runs as a separate step before the server starts
  (§15).
  `serve` refuses to start on an unmigrated database. New migrations MUST
  stay compatible with the running release and MUST NOT break the
  TypeScript revert path until the owner ends it (docs/adr/0001).
- **Queries:** SQLx `query!` macros with committed `.sqlx` metadata. The
  offline tools use runtime queries with bound parameters.
- **Connection:** over the Railway private network. Pool of 10 connections,
  acquire timeout 10 s, `statement_timeout` 30 s. The pool keeps at least 3
  connections open, idle or in use (`min_connections`). A new connection
  costs about 100 ms in production. Without idle connections, the first
  request after 10 idle minutes opened one or two connections and took
  110–250 ms (#150). The cost is 3 PostgreSQL backends that stay open
  between the requests. The other pool options are the SQLx defaults:
  `idle_timeout` 10 min, `max_lifetime` 30 min, and a ping before each
  acquire. The lazy pool keeps the minimum from its first close of an idle
  connection on, about 10–20 min after start.
- **TLS:** SQLx with rustls (`tls-rustls-aws-lc-rs`). With the default
  `sslmode=prefer`, SQLx asks for TLS. It uses plain text only when the
  server declines TLS. When the server accepts, a failed handshake fails
  the connection; there is no fallback to plain text. `prefer` and `require` do not verify the certificate, so
  the self-signed certificate of the Railway image works. `verify-ca` and
  `verify-full` reject that certificate. `migrate` logs a TLS failure as
  `migrate_tls`.
- **`prices`:** upsert by `(delivery_start, area)`. Kept; no pruning.
- **`fingrid_actuals`:** upsert-latest by `(dataset_id, start_time)` with a
  change guard: `DO UPDATE … WHERE (end_time, value) IS DISTINCT FROM
(EXCLUDED.end_time, EXCLUDED.value)`. `fetched_at` means last changed, not
  last seen. Pruned beyond `RETENTION_DAYS` (730).
- **`fingrid_forecasts`:** datasets 245, 165, 166, and 248. Append-only per
  issuance (`ON CONFLICT DO NOTHING`). `issued_at` is the fetch time cut to
  the UTC hour, not the Fingrid publication time; each row is the newest
  Fingrid version at the fetch. An issuance stores only targets with
  `start_time >= issued_at − VINTAGE_BACKFILL_HOURS` (6 h). Pruned by
  `issued_at` beyond `VINTAGE_RETENTION_DAYS` (730), for every dataset,
  after a successful first Fingrid request (§2.3). The served forecast reads
  only 245 and 165; 166 and 248 are archived for offline evaluation. At 730
  days the table holds about 3.3–4.3 GB. Reassess the retention when the
  database reaches 4 GB of the 5 GB Hobby volume limit (#168). **Live
  read shape (do not change):** the latest-per-target read is a LATERAL
  skip-scan on `idx_fingrid_forecasts_target_issued`, not `DISTINCT ON`;
  `DISTINCT ON` sorts every in-range issuance to disk and breaks the §4
  budget.
- **`weather_forecasts`, `weather_daily_forecasts`:** append-only per
  issuance; never collapse to the latest issuance. `target_date` is the UTC
  calendar date of `dt`. Pruned beyond `WEATHER_RETENTION_DAYS` (400). The
  weather-feature work gets its first honest look on 2026-12-13. That date
  is not a kill checkpoint, and the date alone closes nothing. If the work
  is closed later, the daily table is dropped in the same PR.
- **Accounts:** `"user"`, `"account"`, `usernames`, `user_settings`,
  `api_keys` (plaintext keys, `VISION.md`). The Better Auth `"session"` and
  `"verification"` tables stay untouched for the revert path.
- **Recovery:** none for the database. The owner's Railway Hobby plan has
  no database backups, so there is no restore path. The owner accepts the
  data-loss risk (2026-10-19). After a loss, the jobs fetch the public price
  and grid data again within upstream limits. The account, the contract
  settings, and the API key are lost; the owner creates them again, and the
  home automations need the new key.

---

## 6. Approved dependencies

Exact versions are pinned in `Cargo.toml`. The lead approved the set on
2026-10-14 in the Rust port PR.

| Dependency                   | Version            | Why it earns its place                                                                                       |
| ---------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------ |
| axum                         | 0.8.9              | HTTP routing and extraction                                                                                  |
| tokio                        | 1.54.1             | Async runtime, timers, signals, blocking pool                                                                |
| tower-http                   | 0.7.1              | `CatchPanicLayer`, header layers. reqwest 0.13.5 still brings 0.6.11: two copies until reqwest moves to 0.7. |
| tower (dev)                  | 0.5.3              | `ServiceExt::oneshot` in router tests                                                                        |
| serde / serde_json           | 1.0.229 / 1.0.154  | Wire types. `float_roundtrip` parses numbers like JavaScript.                                                |
| thiserror                    | 2.0.21             | Error enums with fixed text                                                                                  |
| jiff / jiff-sqlx             | 0.2.45 / 0.2.2     | Time values with the bundled tzdb, and their SQLx encoding                                                   |
| croner                       | 4.0.3              | Cron schedules with Jiff types                                                                               |
| sqlx                         | 0.9.0              | PostgreSQL, checked macros, migrations, `#[sqlx::test]`, TLS (rustls, aws-lc-rs)                             |
| reqwest                      | 0.13.5             | Upstream requests; rustls, gzip                                                                              |
| futures-util                 | 0.3.34             | `catch_unwind` for jobs; already in the graph                                                                |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23    | Structured logs; the formatter is first-party (§8)                                                           |
| utoipa / utoipa-axum         | 6.0.0 / 0.3.0      | OpenAPI 3.1 document of the routes                                                                           |
| scrypt                       | 0.12.0             | Better Auth password hashes (N=16384, r=16, p=1, 64 bytes)                                                   |
| unicode-normalization        | 0.1.26             | NFKC of a password, like `String.prototype.normalize`                                                        |
| subtle                       | 2.6.1              | Constant-time comparison of a password hash                                                                  |
| sha2                         | 0.11.1             | SHA-256 of a stored session token                                                                            |
| getrandom                    | 0.4.3              | Random tokens, API keys, ids, and salts                                                                      |
| @playwright/test (dev, Node) | see `package.json` | Browser journeys                                                                                             |
| prettier (dev, Node)         | see `package.json` | Format check of Markdown, JSON, YAML, and e2e TypeScript                                                     |

The dev profile builds scrypt, salsa20, pbkdf2, and sha2 with `opt-level =
3`, because an unoptimized scrypt takes seconds per hash in tests.

---

## 7. Stack-specific reject-list additions

The profile list applies. In addition:

- A change of a v1 response, status, header, or error text without a matching
  golden change, an `INTENTIONAL` entry in `golden_http.rs`, and an owner
  decision. A breaking change needs `/api/v2`.
- A migration that the TypeScript revert path cannot run on, while the
  revert path holds (docs/adr/0001).
- A password hash, user row, or account row that Better Auth cannot read
  (docs/adr/0002).
- `DISTINCT ON` in the live latest-vintage read (§5).
- A log field outside `ALLOWED_FIELDS`, added only to see a value.
- Local-time arithmetic below the response boundary; time zone conversion
  outside Jiff.
- An external call without a deadline and a degraded result. An upstream
  failure must never block the price request path.
- A test task that reaches a database other than its own test server, or a
  test that calls a live upstream.

### Code comments

A comment earns its place by stating a **constraint a reader would otherwise
break** — units, ownership, failure behaviour, what a caller must not do. It
does not describe the code. Default to none. A comment runs to at most 5
lines; past that, move the content to the issue, the PR, or this file, and
leave a pointer. Never write history, rationale, rejected alternatives, issue
or PR numbers, line numbers, or counts of things elsewhere. Write for a
reader who has only the source file. Keep an existing comment unless the
change makes it wrong.

---

## 8. Logging & privacy

- `observability.rs` writes one JSON object per line to standard output
  (`LOG_FORMAT=json`, the default) or readable text (`LOG_FORMAT=text`).
  Railway keeps the log.
- Only the fields in `ALLOWED_FIELDS` keep their values. Other fields are
  dropped, and only their names appear in `dropped_fields`.
- One line per request: method, matched route pattern (or `unmatched`),
  status, and time. The raw path, the query string, headers, bodies, keys,
  and IP addresses are never logged.
- A user id is logged only for the `invalid_settings` diagnosis (§2.2).
- The panic hook logs only the source location.
- `Config` debug output redacts `DATABASE_URL`, `BETTER_AUTH_SECRET`, and the
  upstream keys. A configuration error names the setting, never the value.
- No crash reporter, analytics, or telemetry exporter.

---

## 9. Background & lifecycle

`lifecycle::serve` owns the scheduler task. Each job run has a deadline and
`catch_unwind`; a failure or a panic of one job never affects another job
or the request path. Runs of one job never overlap. All jobs run once at
start. These are the only allowed background tasks; a new one needs an
entry here.

| Job                | Schedule                                                                          | Work                                                                                                                                                                                                                                   |
| ------------------ | --------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Day-ahead prices   | `0 */2 * * *` UTC, plus `*/10 12-13 * * *` Europe/Oslo until tomorrow is complete | Nord Pool for today and tomorrow (UTC dates); skip a date when every area has at least `MIN_ENTRIES_PER_AREA` (23) rows. After a run that stored prices, recompute the forecast. Deadline 300 s.                                       |
| Fingrid grid data  | `0 * * * *` UTC, only with `FINGRID_API_KEY`                                      | Actuals (75, 124) upsert and prune; forecast vintages (245, 165) append and prune (§5). After a run that stored data, recompute the forecast. Then archive vintages of 166 and 248 (§2.3); this step never recomputes. Deadline 120 s. |
| OpenWeatherMap     | `0 * * * *` UTC, only with `OPENWEATHERMAP_API_KEY`                               | Hourly and daily blocks for two FI points, stored separately; no pruning when every point failed. About 48 calls per day. Deadline 120 s.                                                                                              |
| Forecast recompute | Once at start (deadline 60 s), then after the price and Fingrid runs above        | Read the forecast inputs (§5), run the model, and swap the result into `ForecastCache` (`std::sync::RwLock<Option<Arc<Snapshot>>>`). A failure keeps the previous result and logs a warning.                                           |

- **Precomputed forecast:** the model result does not depend on the
  user. `GET /api/v1/price/forecast` serves the result of `ForecastCache` and
  applies the contract terms and the time zone of the user per request. The
  recompute runs in the job that triggers it, after its writes; refreshes
  are serialised, so an older build cannot replace a newer result. When the
  cache is empty, or its window came from the clock (no FI price exists), the
  handler computes per request with the same reads and does not store the
  result.
- **Within-day shape of the forecast:** each quarter is the UTC-day mean of
  the model prediction plus the profile price minus the UTC-day mean of the
  profile prices. The profile price is the mean of the same local time on
  the same weekday in each of the last 4 weeks, on Europe/Oslo calendar
  days (`ForecastOptions::served`). Every week must have a price. A quarter
  without all 4 prices keeps the flat daily level: a store gap, or a source
  time in the skipped DST hour. In the repeated DST hour, the source has the
  UTC offset of the target, else the earlier instant. The 30-day price read
  window holds every source day; a compile-time assertion keeps it so.
  `ShapeProfile::Persistence` (24 h earlier, else 168 h) is the rule of the
  TypeScript service. The golden parity tests use it, and the backtest
  option `shape_profile=persistence` selects it.
- **`generatedAt` of a forecast:** the data freshness, not the request
  instant. It is the later of the newest `fingrid_forecasts.issued_at` of
  datasets 245 and 165 and the newest `prices.fetched_at` of FI prices with
  `delivery_start` in `[history_start, series_start)`. When neither exists,
  it is the instant of the computation. The responses without a computation
  (settings not found, area other than FI) keep the request instant.
- **Staleness:** when the newest grid forecast issuance is older than 2 h,
  the recompute and the first serve of that result log a warning with the
  code `forecast_stale`.
- **Grid gaps:** a wind or consumption quarter without a vintage, an
  actual, or an extension value takes the nearest earlier weeks of actuals,
  at most 4 weeks back (docs/adr/0007). A quarter that no week fills reads
  0 MW. The recompute then logs a warning with the code
  `forecast_grid_missing` and the counts `missing_quarters` and
  `missing_training_rows`. The API response does not show it.

- **Shutdown:** SIGTERM or SIGINT stops accepting connections and signals the
  jobs. Active requests get 10 s; jobs get 5 s, then they are aborted; the
  pool gets 5 s; the runtime gets 5 s for blocking tasks.
- **Durable work:** none. A restart loses nothing; the next run fetches
  again.
- **Forbidden:** long-polling connections, idle computation, and any task
  that keeps data that `VISION.md` forbids.

---

## 10. Time & timezones

- **Instant:** `jiff::Timestamp`; PostgreSQL `timestamptz`; on the wire
  `YYYY-MM-DDTHH:MM:SS.sssZ`.
- **Calendar value:** a delivery day is a local date in the user's IANA zone
  (`user_settings.timezone`, one of the 13 supported zones). `time.rs` turns
  it into a UTC range with `Zoned::start_of_day`, so 23-hour and 25-hour days
  are correct. The night window is a recurring local hour range, evaluated
  per instant.
- **Duration:** a number with the unit in its name (`duration_minutes`,
  `RETENTION_DAYS`). Never assume 24 hours per local day.
- **Conversion:** only through Jiff with the bundled time zone database. The
  database updates with the jiff crate version.
- **Tests:** pass fixed time (`FixedClock`, explicit `now`). Golden files
  cover every supported zone on every day of 2026 and around the 2025 DST
  changes. Results do not depend on the host time zone.

---

## 11. Design guidelines & UX thresholds

The web UI is a setup surface only (`VISION.md`). It is the page of the
TypeScript service, except the chart drawing. It must be usable without a pointer.

- **Design authority:** WCAG 2.2 AA and native HTML semantics.
- **Thresholds:** pointer target size ≥ 24×24 CSS px (WCAG 2.5.8); text
  contrast ≥ 4.5:1 and ≥ 3:1 for large text (WCAG 1.4.3); a visible focus
  indicator on every interactive element (WCAG 2.4.7).
- **Input paths:** full keyboard operation; focus order follows DOM order.
- **Browser evidence:** the Playwright journeys in `e2e/` (Chromium) run
  against the Rust binary with seeded data and no live upstream calls.
- **Copy:** terse and technical, as in `VISION.md → Audience & Voice`.

---

## 12. Best practices source

- [Axum 0.8](https://docs.rs/axum/0.8.9/), [tower-http 0.7](https://docs.rs/tower-http/0.7.1/), [Tokio shutdown](https://tokio.rs/tokio/topics/shutdown)
- [SQLx 0.9](https://docs.rs/sqlx/0.9.0/), [jiff-sqlx](https://docs.rs/jiff-sqlx/0.2.2/), [PostgreSQL 18](https://www.postgresql.org/docs/18/)
- [Jiff](https://docs.rs/jiff/0.2.45/), [croner 4](https://docs.rs/croner/4.0.3/), [reqwest 0.13](https://docs.rs/reqwest/0.13.5/), [utoipa](https://docs.rs/utoipa/6.0.0/)
- [scrypt](https://docs.rs/scrypt/0.12.0/), [ECMAScript Number::toString](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
- Upstream contracts: Nord Pool Data Portal API, [Fingrid open data](https://data.fingrid.fi/), [OpenWeatherMap One Call 3.0](https://openweathermap.org/api/one-call-3)
- [Railpack Rust](https://railpack.com/languages/rust/), [Railpack config file](https://railpack.com/config/file/), Railway [healthchecks](https://docs.railway.com/deployments/healthchecks)
- [WCAG 2.2](https://www.w3.org/TR/WCAG22/), [Playwright](https://playwright.dev/)

---

## 13. Scoped exceptions

| Rule / scope                                                                                                                                                                                          | Reason and consequences                                                                                                                                                                                                                                                                                                                             | Compensating evidence                                                                                                          | Lead / reviewer / approval                                                             | Expiry or reassessment                                                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Profile §15 build isolation: Railpack gives every Railway service variable, including `DATABASE_URL`, `BETTER_AUTH_SECRET`, and the upstream keys, to every build command and dependency build script | `railpack.json` sets `"secrets": []` for the install and build steps. The Railpack documentation says that this limits the secrets of a step, but the owner's other services found that it limits only cache invalidation. This file assumes that every build step can see every variable. The same applies to the owner's other Railpack services. | Committed `Cargo.lock` with `--locked`; cargo-deny (sources, bans, advisories); a small dependency set with a reason each (§6) | Lead; approved by the independent reviewer of the port PR (2026-10-14, review round 2) | When a dependency with a build script or proc macro is added, or on a supply-chain advisory |
| Profile §15 dependency-aware readiness: `/health` checks only `SELECT 1`                                                                                                                              | The TypeScript contract of `/health` stays. The start command runs `migrate` first, so a broken database stops the deploy before traffic moves.                                                                                                                                                                                                     | Migration and health tests; §15 healthcheck                                                                                    | Lead; approved by the independent reviewer of the port PR (2026-10-14, review round 2) | When the service gets another dependency that a deploy must check                           |
| Forecast parity: values before rounding differ from V8 by a few ULPs                                                                                                                                  | Rust uses the platform maths library for `asinh`, `sinh`, `exp`, `sin`, `cos`. A residual exactly on a band edge can flip the observed band coverage of the offline regeneration.                                                                                                                                                                   | Golden tests: relative tolerance 1e-9 before rounding, exact after rounding; the band-edge count is asserted                   | Lead; approved by the independent reviewer of the port PR (2026-10-14, review round 2) | When the bands are calibrated and shipped                                                   |

---

## 14. Applicability and evidence

Run the required tests and the full `mise run verify` locally on the version
to be merged against the current base. Missing or failed mandatory local
evidence blocks merge. There is no CI.

| Doctrine / applicability      | Required evidence                                                                                                        | Environment / gap                                                            |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------- |
| P1, P6: each task             | Criteria, failures, assumptions, check mapping in the PR                                                                 | Lead records; independent review for material changes                        |
| P2–P4: types and dependencies | Clippy, cargo-deny, cargo-shear; dependency reasons (§6); crate boundary                                                 | Purity inside the domain crate is review-only                                |
| P3, P5: API contract          | HTTP golden replay; `contract:check`; OpenAPI comparison                                                                 | Golden files come from one TypeScript commit                                 |
| P5: data meaning              | Domain golden files (price math, local days, queries, forecast)                                                          | Forecast tolerance (§13)                                                     |
| P5–P7: storage                | `sqlx:check`; migration tests (TS schema, empty, repeat, partial ledger, failure kinds); snapshot; precheck; store tests | PostgreSQL 18.7 test server (Apple container); production: `postgres-ssl:18` |
| P3, P7: security              | Gitleaks; cargo-deny; `pnpm audit`; password vector tests; session expiry, renewal, sign-out, and Origin tests           | Scanners do not prove authorization                                          |
| P5–P7: browser                | Playwright journeys against the Rust binary                                                                              | Chromium only; no axe check                                                  |
| P7: release                   | Before merge: Railpack plan; owner-run schema snapshot and precheck when §15 asks for them. After release: §15 checks    | A local macOS build is not the Linux artifact                                |
| P8–P9: material change        | Independent review with the reviewer's own `mise run verify`                                                             | Separate reviewer context                                                    |

### Golden files

`crates/server/tests/golden/http/README.md`, the domain golden files, and the
tools golden files record how they were produced from the TypeScript
service. The TypeScript service is gone from the tree; regenerate them only
from the port PR's history. Exception: the synthetic backtest sections of
`crates/tools/tests/golden/backtest_expected.json` come from the Rust tools,
because the backtest now follows the served forecast. Their README gives the
regeneration command. The HTTP forecast bodies stay TypeScript output; their
estimate numbers differ through `forecast-values` entries (§2.1). A golden
change is a contract change.

---

## 15. Release, recovery, and maintenance

The release is one always-on Railway service (`spot-price` in project
`calmdonut`, environment `production`, 1 replica) built by Railpack from
`railpack.json`. Each merge to `main` deploys automatically. The owner
reserves merge (`CLAUDE.md`).

### Build and start (in the repository)

| Item          | Value                                                                                                                                                   |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Builder       | Railpack, Rust provider (`"provider": "rust"`: the root also has `package.json`)                                                                        |
| Toolchain     | Rust 1.100.0 from `rust-toolchain.toml`                                                                                                                 |
| Build         | One command: remove the old binary, `SQLX_OFFLINE=true cargo build --release --locked --package spot-price-server`, copy it to `bin/`                   |
| Caches        | `cargo_registry`, `cargo_git`, `cargo_target` (`"type": "locked"`: one build at a time)                                                                 |
| Image content | `bin/spot-price` only                                                                                                                                   |
| Start         | `./bin/spot-price migrate && exec ./bin/spot-price serve`. The entrypoint is `/bin/bash -c`, so after `exec` the service is PID 1 and receives SIGTERM. |
| Build context | Without `mise.toml` (local tools only)                                                                                                                  |

The removal, the build, and the copy run in one command. `target` is a
Railpack cache mount, and a separate copy command can see a different
cache state than the build: no binary, or an old binary from another
build. An old binary can lack a migration, and `migrate` then stops with
`migrate_version`. The `cargo_target` cache is `locked`, so an overlapping
build of the service cannot replace the binary between the build and the
copy.

`crates/server/build.rs` prints `cargo:rerun-if-changed=../../migrations`.
`sqlx::migrate!` embeds the migrations, and without the build script Cargo
does not rebuild the server for a change that only adds a migration file.
Manual check: add a file to `migrations/` and run `cargo build --package
spot-price-server`; Cargo must print `Compiling spot-price-server`.

`SQLX_OFFLINE=true` is required: the build gets the service variable
`DATABASE_URL`, and the SQLx macros would otherwise connect to the
production database. Railpack cannot read inherited package fields in the
binary crate, so `crates/server/Cargo.toml` repeats them. Check `railpack
info --show-plan .` after a change to a manifest or to `railpack.json`.

### Service settings (in Railway)

| Setting       | Value                                                 |
| ------------- | ----------------------------------------------------- |
| Healthcheck   | `/health`, timeout 30 s                               |
| Memory limit  | 375 MB                                                |
| Custom domain | `spot.calmdonut.com` (port 8080); Railway sets `PORT` |

### Runtime variables (names only)

- Required in production: `DATABASE_URL`, `BETTER_AUTH_URL` (the public
  origin; it sets the cookie security and the `Origin` check),
  `BETTER_AUTH_SECRET` (unused by the Rust service; kept for the revert
  path), `NODE_ENV=production`.
- Optional: `FINGRID_API_KEY`, `OPENWEATHERMAP_API_KEY` (a missing key
  disables its job and logs a warning), `LOG_FORMAT`.
- Platform: `PORT`.
- An invalid value stops the start with exit code 2. The message names the
  setting, never its value.

### Release procedure

1. Record the active deployment and its commit in the PR.
2. Before the merge of a release that adds a migration, or that changes the
   database connection, `BETTER_AUTH_URL`, or the start command, the owner
   checks production read-only:
   - Run both scripts with the public database URL. `psql` runs in a
     throw-away container of the pinned image; the Mac has no `psql`:

     ```bash
     pgc() { container run --rm --interactive --progress none \
       --env PGOPTIONS='-c default_transaction_read_only=on' \
       "$(mise env --json | jq -r .SPOT_PRICE_PG_IMAGE)" psql "$@"; }
     pgc "$DATABASE_PUBLIC_URL" -X -q -A -t -f - <scripts/schema-snapshot.sql \
       | diff crates/server/tests/fixtures/schema-snapshot.txt -
     pgc "$DATABASE_PUBLIC_URL" -X -q -A -t -f - <scripts/release-precheck.sql
     ```

   - The snapshot differs only by the Rust migrations that production
     already has. Through migration 3, that is the three indexes that
     migration 3 drops (§5). The precheck prints
     `typescript_migrations|1,2,3,4,5,6,7`, the server version, and `0` for
     each `user_settings_invalid_*` count. A non-zero count is a user for
     whom the Rust service answers 500. It blocks the merge until the owner
     decides how to correct the row.
   - `DATABASE_URL` of the service has no `sslmode`, or `sslmode=prefer` or
     `sslmode=disable` (§5). The service has no `PGSSLMODE` variable, or it
     is `prefer` or `disable`: SQLx reads it when the URL has no `sslmode`.
   - The server certificate is version 3. SQLx with rustls rejects a
     version 1 certificate, and `psql` does not show the difference:
     `openssl s_client -starttls postgres -connect <public host>:<port>
</dev/null 2>/dev/null | openssl x509 -noout -text | grep Version`.
   - `BETTER_AUTH_URL` is exactly `https://spot.calmdonut.com`: no trailing
     slash, no path. The `Origin` check compares it byte for byte.

3. Merge. Watch the build log: the Railpack version, Rust 1.100.0, the
   offline build, `migrations are up to date`, and a passing healthcheck.
4. Run the post-release checks.

### Post-release checks (read-only)

- The active deployment shows the merged commit.
- `GET https://spot.calmdonut.com/health` gives `{"status":"ok","db":"connected"}`.
- The next scheduled price fetch appears in the log without errors.
- The owner calls one price route with their own API key.

Never run `scripts/smoke-local.sh` against production: it creates an account
and an API key.

### Rollback and recovery

- **Bad release:** fix it with a revert PR or a fix PR, through the normal
  review, merge, and release path. Owner decision (2026-10-19): do not use
  the Railway rollback after a migration. A deployment that does not know
  every applied migration stops at `migrate` (`migrate_version`). A failed
  build or a failed start leaves the active deployment in service.
- **Code:** `git revert -m 1 <merge commit>` on a branch, then merge. A
  revert of the port removes `railpack.json`, so Railpack detects the Node
  service again. A revert of a later change keeps its migration files.
- **Data:** a revert needs no data change (docs/adr/0001). A revert to a
  180-day vintage retention, including a revert to the TypeScript service,
  deletes every Fingrid forecast vintage older than 180 days at its first
  Fingrid run. That deletion cannot be undone. Sessions created
  by the Rust service end at a revert; users sign in again. `_sqlx_migrations` and `app_sessions` stay and are
  harmless.
- **Database loss:** no backup and no restore path (§5).

### Maintenance

- **Infrastructure:**

  | Component      | Production                                                                                        | Test and local                                                                     | How to read the version                                                               |
  | -------------- | ------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
  | PostgreSQL     | Railway image `ghcr.io/railwayapp-templates/postgres-ssl:18`; the minor version follows the image | Image `postgres:18.7` in an Apple container (`SPOT_PRICE_PG_IMAGE` in `mise.toml`) | Production: `SHOW server_version`, read-only, by the owner. Test: `mise run db:info`. |
  | Source builder | Railpack; Railway selects the version                                                             | Not used                                                                           | The Railway build log                                                                 |
  | Runtime image  | Railpack runtime image; Railway selects the version                                               | Not used                                                                           | The Railway build log                                                                 |
  | Rust toolchain | 1.100.0 (`rust-toolchain.toml`)                                                                   | 1.100.0                                                                            | `rust-toolchain.toml`                                                                 |

  Keep test and production PostgreSQL on the same major version (§5).

  Do not pin Railpack or its runtime image. Do not set `RAILPACK_VERSION`
  on the service. Each build uses the Railpack version that Railway selects
  by default. That default can be older than the latest Railpack release;
  the build log shows the version. A failed build leaves the active
  deployment in service. When a new Railpack version breaks the build, fix
  the repository in a `fix/` PR. When the new image breaks the running
  service, fix it with a revert PR or a fix PR (Rollback and recovery).

- **Dependencies:** apply `DOCTRINE.md` P7. The minimum release age is 1440
  minutes. `pnpm-workspace.yaml` enforces it for npm packages. For crates and
  mise tools, check the release date by hand during the freshness check.
  Exclude a version from the age rule only for a security fix. These pins
  change together: Node in `mise.toml`, `.nvmrc`, and `package.json`
  `engines`, with `@types/node` on the same major version; pnpm in
  `mise.toml` and `packageManager`; `sqlx`, `sqlx-cli`, and `jiff-sqlx`. A
  new Node major version follows the Node LTS line.

Review toolchain and dependency upgrades with the same evidence. After a
query or migration change, run `mise run sqlx:prepare` and commit `.sqlx`.
After a Railpack change, compare the plan again. Keep the TypeScript revert
path until the owner ends it.

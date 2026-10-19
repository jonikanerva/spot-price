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
  failures are recoverable by a rollback in minutes. A destructive migration
  is the main irreversible risk.
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
  tools/               # offline forecast tools (backtest, regenerate-bands, revision-magnitude)
migrations/            # SQLx migrations
.sqlx/                 # committed SQLx query metadata
contract/openapi.json  # exported OpenAPI 3.1 document
e2e/                   # Playwright journeys against the Rust binary
scripts/               # test-db.sh, smoke-local.sh, schema-snapshot.sql, golden generators (history only)
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

- **Language:** Rust 1.99.1, edition 2024, pinned in `rust-toolchain.toml`
  with rustfmt and Clippy. `rust-version = "1.99"` in the workspace; the
  server crate repeats it (§15, Railpack).
- **Runtime:** a native Linux binary in the Railpack runtime image. A local
  macOS build does not validate the Linux artifact.
- **Package manager:** Cargo with workspace dependency inheritance, committed
  `Cargo.lock`, `--locked` on every task.
- **Strictness:** the workspace lints of the profile (`unsafe_code =
"forbid"`, Clippy pedantic, and the deny list) plus
  `missing_debug_implementations`. `clippy.toml` allows unwrap, expect,
  panic, and print in tests.
- **Node.js:** Node 24 and pnpm 12 run only Playwright and Prettier. They
  are not in the production image.

### Provisioning

Install mise and rustup. Then run in the repository root:

```bash
mise trust && mise run setup
```

`rust-toolchain.toml` owns the Rust version. `mise.toml` owns the other
tools: Node, pnpm, PostgreSQL 18.6 (conda package), sqlx-cli 0.9.0,
cargo-deny, cargo-nextest, cargo-shear, and Gitleaks.

---

## 2. Frameworks

| Concern             | Technology                                   | Role                                           |
| ------------------- | -------------------------------------------- | ---------------------------------------------- |
| HTTP                | Axum 0.8, tower-http 0.6                     | Routes, extraction, panic containment          |
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
  (`LatestPublishedRule::Before`) differs from the TypeScript service.
  The `generated-at` kind sets a differing `generatedAt` of a forecast body
  to the recorded value before the other body checks, so one step can list
  both `generated-at` and `forecast-values`.
- `contract/openapi.json` is the export of `spot-price export-openapi`. The
  `contract:check` task fails on drift. `contract.rs` compares it with the
  TypeScript document field by field.
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
- **Fingrid:** `GET data.fingrid.fi/api/data`, datasets 245, 75, 165, 124,
  header `x-api-key`; no retry, never throws.
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
- **test:** starts the isolated database, then `cargo nextest run
--workspace --locked` and `cargo test --doc --workspace --locked`.
- **e2e:** the Playwright journeys in `e2e/` against the Rust binary.
- **verify:** format-check, lint, `sqlx:check`, `contract:check`,
  `scan:secrets`, `scan:deps`, build, test, and e2e, in this order. It stops
  at the first failed step. The Playwright journeys are part of `verify`.
- **Other tasks:** `db:start` / `db:stop`, `sqlx:prepare` (regenerate
  `.sqlx` after a query or migration change), and `setup`.
- **Dependency freshness check** (`DOCTRINE.md` P7, read-only): run
  `cargo update --dry-run --verbose`, `pnpm outdated --recursive`, and
  `mise outdated --bump`. Each crate pin in `Cargo.toml` is exact, so also
  compare each direct crate with its latest stable release on crates.io.
  Compare `rust-toolchain.toml` with the latest stable Rust release. Compare
  each infrastructure version in §15 with its latest stable release. Record
  the release date of each candidate version (§15, Dependencies). The checks
  need network access to crates.io, npm, GitHub, and the mise version
  service. A sandbox that blocks them gives no result; that is a gap, not a
  pass.

**Test database isolation.** `scripts/test-db.sh` runs the pinned PostgreSQL
18.6 on `127.0.0.1:55436` with trust authentication and data in
`.tmp/postgres`. The test tasks set `DATABASE_URL` to that server themselves.
`#[sqlx::test]` creates and drops one database per test.

**No `.env` file.** The SQLx query macros parse every `.env` file in the
crate directory and its parent directories at compile time. Keep local
settings in `.env.local`. sqlx-cli runs with `--no-dotenv`.

**Sandboxes.** PostgreSQL needs System V shared memory, and the tests bind
`127.0.0.1` ports. A sandbox that blocks either makes `db:start` or the tests
fail; that is a missing environment, not a pass.

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
  acquire timeout 10 s, `statement_timeout` 30 s.
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
- **`fingrid_forecasts`:** append-only per issuance (`ON CONFLICT DO
NOTHING`). An issuance stores only targets with `start_time >= issued_at −
VINTAGE_BACKFILL_HOURS` (6 h). Pruned by `issued_at` beyond
  `VINTAGE_RETENTION_DAYS` (180). **Live read shape (do not change):** the
  latest-per-target read is a LATERAL skip-scan on
  `idx_fingrid_forecasts_target_issued`, not `DISTINCT ON`; `DISTINCT ON`
  sorts every in-range issuance to disk and breaks the §4 budget.
- **`weather_forecasts`, `weather_daily_forecasts`:** append-only per
  issuance; never collapse to the latest issuance. `target_date` is the UTC
  calendar date of `dt`. Pruned beyond `WEATHER_RETENTION_DAYS` (400). The
  weather-feature work gets its first honest look on 2026-12-13. That date
  is not a kill checkpoint, and the date alone closes nothing. If the work
  is closed later, the daily table is dropped in the same PR.
- **Accounts:** `"user"`, `"account"`, `usernames`, `user_settings`,
  `api_keys` (plaintext keys, `VISION.md`). The Better Auth `"session"` and
  `"verification"` tables stay untouched for the revert path.
- **Recovery:** Railway PostgreSQL backups. Public data can be fetched again
  within upstream limits.

---

## 6. Approved dependencies

Exact versions are pinned in `Cargo.toml`. The lead approved the set on
2026-10-14 in the Rust port PR.

| Dependency                   | Version            | Why it earns its place                                                             |
| ---------------------------- | ------------------ | ---------------------------------------------------------------------------------- |
| axum                         | 0.8.9              | HTTP routing and extraction                                                        |
| tokio                        | 1.53.3             | Async runtime, timers, signals, blocking pool                                      |
| tower-http                   | 0.6.11             | `CatchPanicLayer`, header layers. 0.6, not 0.7: reqwest already depends on 0.6.11. |
| tower (dev)                  | 0.5.3              | `ServiceExt::oneshot` in router tests                                              |
| serde / serde_json           | 1.0.229 / 1.0.154  | Wire types. `float_roundtrip` parses numbers like JavaScript.                      |
| thiserror                    | 2.0.21             | Error enums with fixed text                                                        |
| jiff / jiff-sqlx             | 0.2.38 / 0.2.0     | Time values with the bundled tzdb, and their SQLx encoding                         |
| croner                       | 4.0.1              | Cron schedules with Jiff types                                                     |
| sqlx                         | 0.9.0              | PostgreSQL, checked macros, migrations, `#[sqlx::test]`, TLS (rustls, aws-lc-rs)   |
| reqwest                      | 0.13.5             | Upstream requests; rustls, gzip                                                    |
| futures-util                 | 0.3.34             | `catch_unwind` for jobs; already in the graph                                      |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23    | Structured logs; the formatter is first-party (§8)                                 |
| utoipa / utoipa-axum         | 6.0.0 / 0.3.0      | OpenAPI 3.1 document of the routes                                                 |
| scrypt                       | 0.12.0             | Better Auth password hashes (N=16384, r=16, p=1, 64 bytes)                         |
| unicode-normalization        | 0.1.26             | NFKC of a password, like `String.prototype.normalize`                              |
| subtle                       | 2.6.1              | Constant-time comparison of a password hash                                        |
| sha2                         | 0.11.1             | SHA-256 of a stored session token                                                  |
| getrandom                    | 0.4.3              | Random tokens, API keys, ids, and salts                                            |
| @playwright/test (dev, Node) | see `package.json` | Browser journeys                                                                   |
| prettier (dev, Node)         | see `package.json` | Format check of Markdown, JSON, YAML, and e2e TypeScript                           |

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
- A test task that reaches a database other than the isolated server, or a
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

| Job                | Schedule                                                                          | Work                                                                                                                                                                                             |
| ------------------ | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Day-ahead prices   | `0 */2 * * *` UTC, plus `*/10 12-13 * * *` Europe/Oslo until tomorrow is complete | Nord Pool for today and tomorrow (UTC dates); skip a date when every area has at least `MIN_ENTRIES_PER_AREA` (23) rows. After a run that stored prices, recompute the forecast. Deadline 300 s. |
| Fingrid grid data  | `0 * * * *` UTC, only with `FINGRID_API_KEY`                                      | Actuals (75, 124) upsert and prune; forecast vintages (245, 165) append and prune (§5). After a run that stored data, recompute the forecast. Deadline 120 s.                                    |
| OpenWeatherMap     | `0 * * * *` UTC, only with `OPENWEATHERMAP_API_KEY`                               | Hourly and daily blocks for two FI points, stored separately; no pruning when every point failed. About 48 calls per day. Deadline 120 s.                                                        |
| Forecast recompute | Once at start (deadline 60 s), then after the price and Fingrid runs above        | Read the forecast inputs (§5), run the model, and swap the result into `ForecastCache` (`std::sync::RwLock<Option<Arc<Snapshot>>>`). A failure keeps the previous result and logs a warning.     |

- **Precomputed forecast:** the model result does not depend on the
  user. `GET /api/v1/price/forecast` serves the result of `ForecastCache` and
  applies the contract terms and the time zone of the user per request. The
  recompute runs in the job that triggers it, after its writes; refreshes
  are serialised, so an older build cannot replace a newer result. When the
  cache is empty, or its window came from the clock (no FI price exists), the
  handler computes per request with the same reads and does not store the
  result.
- **`generatedAt` of a forecast:** the data freshness, not the request
  instant. It is the later of the newest `fingrid_forecasts.issued_at` of
  datasets 245 and 165 and the newest `prices.fetched_at` of FI prices with
  `delivery_start` in `[history_start, series_start)`. When neither exists,
  it is the instant of the computation. The responses without a computation
  (settings not found, area other than FI) keep the request instant.
- **Staleness:** when the newest grid forecast issuance is older than 2 h,
  the recompute and the first serve of that result log a warning with the
  code `forecast_stale`.

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

- [Axum 0.8](https://docs.rs/axum/0.8.9/), [tower-http 0.6](https://docs.rs/tower-http/0.6.11/), [Tokio shutdown](https://tokio.rs/tokio/topics/shutdown)
- [SQLx 0.9](https://docs.rs/sqlx/0.9.0/), [jiff-sqlx](https://docs.rs/jiff-sqlx/0.2.0/), [PostgreSQL 18](https://www.postgresql.org/docs/18/)
- [Jiff](https://docs.rs/jiff/0.2.38/), [croner 4](https://docs.rs/croner/4.0.1/), [reqwest 0.13](https://docs.rs/reqwest/0.13.5/), [utoipa](https://docs.rs/utoipa/6.0.0/)
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

| Doctrine / applicability      | Required evidence                                                                                                        | Environment / gap                                       |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| P1, P6: each task             | Criteria, failures, assumptions, check mapping in the PR                                                                 | Lead records; independent review for material changes   |
| P2–P4: types and dependencies | Clippy, cargo-deny, cargo-shear; dependency reasons (§6); crate boundary                                                 | Purity inside the domain crate is review-only           |
| P3, P5: API contract          | HTTP golden replay; `contract:check`; OpenAPI comparison                                                                 | Golden files come from one TypeScript commit            |
| P5: data meaning              | Domain golden files (price math, local days, queries, forecast)                                                          | Forecast tolerance (§13)                                |
| P5–P7: storage                | `sqlx:check`; migration tests (TS schema, empty, repeat, partial ledger, failure kinds); snapshot; precheck; store tests | Isolated PostgreSQL 18.6; production: `postgres-ssl:18` |
| P3, P7: security              | Gitleaks; cargo-deny; `pnpm audit`; password vector tests; session expiry, renewal, sign-out, and Origin tests           | Scanners do not prove authorization                     |
| P5–P7: browser                | Playwright journeys against the Rust binary                                                                              | Chromium only; no axe check                             |
| P7: release                   | Before merge: Railpack plan; owner-run schema snapshot and precheck when §15 asks for them. After release: §15 checks    | A local macOS build is not the Linux artifact           |
| P8–P9: material change        | Independent review with the reviewer's own `mise run verify`                                                             | Separate reviewer context                               |

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
| Toolchain     | Rust 1.99.1 from `rust-toolchain.toml`                                                                                                                  |
| Build         | `SQLX_OFFLINE=true cargo build --release --locked --package spot-price-server`, then copy the binary to `bin/`                                          |
| Caches        | `cargo_registry`, `cargo_git`, `cargo_target`                                                                                                           |
| Image content | `bin/spot-price` only                                                                                                                                   |
| Start         | `./bin/spot-price migrate && exec ./bin/spot-price serve`. The entrypoint is `/bin/bash -c`, so after `exec` the service is PID 1 and receives SIGTERM. |
| Build context | Without `mise.toml` (local tools only)                                                                                                                  |

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

1. Record the active deployment as the known-good version.
2. Before the merge of a release that adds a migration, or that changes the
   database connection, `BETTER_AUTH_URL`, or the start command, the owner
   checks production read-only:
   - Run both scripts with the public database URL:

     ```bash
     PGOPTIONS='-c default_transaction_read_only=on' \
       psql "$DATABASE_PUBLIC_URL" -X -q -A -t -f scripts/schema-snapshot.sql \
       | diff crates/server/tests/fixtures/schema-snapshot.txt -
     PGOPTIONS='-c default_transaction_read_only=on' \
       psql "$DATABASE_PUBLIC_URL" -X -q -A -t -f scripts/release-precheck.sql
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

3. Merge. Watch the build log: the Railpack version, Rust 1.99.1, the
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

- **Immediate:** roll back to the known-good deployment in Railway. A Rust
  deployment that does not know every applied migration stops at `migrate`
  (`migrate_version`), and Railway keeps the active deployment. So after
  migration 3, a rollback to a Rust deployment before it fails. Then use a
  forward fix, a code revert that keeps the migration files, or the
  TypeScript revert.
- **Code:** `git revert -m 1 <merge commit>` on a branch, then merge. A
  revert of the port removes `railpack.json`, so Railpack detects the Node
  service again. A revert of a later change keeps its migration files.
- **Data:** a revert needs no data change (docs/adr/0001). Sessions created
  by the Rust service end at a revert; users sign in again. `_sqlx_migrations` and `app_sessions` stay and are
  harmless.
- **Database loss:** restore a Railway PostgreSQL backup.

### Maintenance

- **Infrastructure:**

  | Component      | Production                                                                                        | Test and local                         | How to read the version                                                        |
  | -------------- | ------------------------------------------------------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------ |
  | PostgreSQL     | Railway image `ghcr.io/railwayapp-templates/postgres-ssl:18`; the minor version follows the image | 18.6 (`mise.toml`, `conda:postgresql`) | Production: `SHOW server_version`, read-only, by the owner. Test: `mise.toml`. |
  | Source builder | Railpack; Railway selects the version                                                             | Not used                               | The Railway build log                                                          |
  | Runtime image  | Railpack runtime image; Railway selects the version                                               | Not used                               | The Railway build log                                                          |
  | Rust toolchain | 1.99.1 (`rust-toolchain.toml`)                                                                    | 1.99.1                                 | `rust-toolchain.toml`                                                          |

  Keep test and production PostgreSQL on the same major version (§5).

  Do not pin Railpack or its runtime image. Do not set `RAILPACK_VERSION`
  on the service. Each build uses the Railpack version that Railway selects
  by default. That default can be older than the latest Railpack release;
  the build log shows the version. A failed build leaves the active
  deployment in service. When a new Railpack version breaks the build, fix
  the repository in a `fix/` PR. When the new image breaks the running
  service, roll back first (Rollback and recovery), then fix the repository.

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

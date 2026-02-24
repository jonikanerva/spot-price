# Implementation Plan: Spot-Price MVP

Date: 2026-02-24
Author: RPI Orchestrator
Status: Draft — awaiting owner review
Research basis: `docs/research/2026-02-24-foundation-research.md` (Approved 2026-02-24)

---

## Goal

Deliver a working MVP of Spot Price: a web service where Finnish spot-electricity customers configure their contract, view total prices, and call a REST API for current price and cheapest window — deployed on Railway.

## Scope Boundaries

**In scope (MVP):**

- Project scaffolding (TypeScript, Hono, SQLite, tooling)
- Database schema and migrations (users, settings, prices, API keys)
- Nord Pool Data Portal price ingestion with daily cron
- Total price calculation (spot + margin + transfer day/night + electricity tax + VAT)
- REST API: current total price
- REST API: cheapest contiguous N-minute window
- Auth: signup/login with Better Auth, API key generation
- Minimal web UI: login, contract settings, price chart, API key management
- Railway deployment with SQLite volume
- Health check endpoint

**Out of scope (documented in VISION.md):**

- Multi-area Nordpool support
- Consumption tracking/forecasting
- Push notifications, payments, mobile app

---

## Milestones

### M0: Project Scaffolding

**Goal:** Runnable TypeScript project with tooling, CI checks pass on empty app.

| Task | Detail                                                                                  | Acceptance                                           |
| ---- | --------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| M0.1 | Init Node.js project with `package.json`, TypeScript strict config                      | `tsc --noEmit` passes                                |
| M0.2 | Configure ESLint (flat config), Prettier                                                | `npm run lint` and `npm run format:check` pass       |
| M0.3 | Set up Hono with `@hono/node-server`, hello-world route                                 | `GET /health` returns 200                            |
| M0.4 | Set up better-sqlite3 with WAL mode, DB init on startup                                 | App starts, creates DB file if absent                |
| M0.5 | Configure Vitest for testing                                                            | `npm test` runs and passes (with 1 placeholder test) |
| M0.6 | Add `tsup` build for production bundle                                                  | `npm run build && node dist/index.js` starts server  |
| M0.7 | Add scripts: `dev` (tsx watch), `build`, `start`, `lint`, `format`, `test`, `typecheck` | All scripts work                                     |

**Dependencies:** None
**Estimated effort:** 1 session (~2–3 hours)

---

### M1: Database Schema & Migrations

**Goal:** SQLite schema supports all MVP data: users, settings, prices, API keys.

| Task | Detail                                                                                                                                                                                                                                                                                          | Acceptance                                |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------- |
| M1.1 | Design and implement migration system (SQL files, version tracking)                                                                                                                                                                                                                             | Migrations run on startup, are idempotent |
| M1.2 | Create `prices` table: `(id, delivery_start TEXT, delivery_end TEXT, price_eur_mwh REAL, area TEXT DEFAULT 'FI', fetched_at TEXT)`                                                                                                                                                              | Table exists after migration              |
| M1.3 | Create Better Auth tables (users, sessions, accounts) via Better Auth's auto-migration                                                                                                                                                                                                          | Auth tables exist after startup           |
| M1.4 | Create `user_settings` table: `(user_id TEXT PK, margin_cents_kwh REAL, transfer_day_cents_kwh REAL, transfer_night_cents_kwh REAL, tax_cents_kwh REAL, vat_percent REAL DEFAULT 25.5, night_start_hour INT DEFAULT 22, night_end_hour INT DEFAULT 7, timezone TEXT DEFAULT 'Europe/Helsinki')` | Table exists, defaults work               |
| M1.5 | Create `api_keys` table: `(id TEXT PK, user_id TEXT, key_hash TEXT, name TEXT, created_at TEXT, last_used_at TEXT)`                                                                                                                                                                             | Table exists                              |
| M1.6 | Write seed data utility for development (sample prices)                                                                                                                                                                                                                                         | `npm run seed` populates test data        |

**Dependencies:** M0 complete
**Estimated effort:** 1 session (~2 hours)

---

### M2: Price Ingestion

**Goal:** Fetch Finnish spot prices from Nord Pool Data Portal API, store in SQLite, run daily via cron.

| Task | Detail                                                                                                            | Acceptance                                           |
| ---- | ----------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| M2.1 | Implement Nord Pool Data Portal client: `fetchDayAheadPrices(date: string, area: string): Promise<HourlyPrice[]>` | Unit test: parses real API response shape correctly  |
| M2.2 | EUR/MWh → c/kWh conversion utility                                                                                | Unit test: `110.5 EUR/MWh → 11.05 c/kWh`             |
| M2.3 | Price storage: upsert hourly prices into `prices` table (idempotent)                                              | Unit test: duplicate inserts don't create duplicates |
| M2.4 | Daily fetch job: fetch today + tomorrow (when available), store results                                           | Integration test: fetches and stores prices          |
| M2.5 | node-cron scheduler: run at `0 12 * * *` UTC (14:00 EET winter), retry every 15 min for 2 hours on failure        | Cron fires at correct time; retry logic works        |
| M2.6 | Startup fetch: on server start, fetch today's prices if not already in DB                                         | App self-heals after restart                         |
| M2.7 | Logging: log fetch results (count, date range, success/failure)                                                   | Fetch results visible in stdout                      |

**Dependencies:** M1 complete
**Estimated effort:** 1 session (~3 hours)

---

### M3: Total Price Calculation Engine

**Goal:** Pure function that computes total price from spot price + user settings. This is the core business logic.

| Task | Detail                                                                                                       | Acceptance                                                        |
| ---- | ------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------- |
| M3.1 | Implement `calculateTotalPrice(spotCentsKwh, settings, deliveryHour): TotalPrice`                            | Returns breakdown: spot + margin + transfer + tax + VAT           |
| M3.2 | Day/night transfer rate logic: use `night_start_hour`, `night_end_hour` to select correct rate               | Unit test: 22:00–07:00 uses night rate, others use day rate       |
| M3.3 | VAT calculation: apply VAT% to (spot + margin + transfer + tax)                                              | Unit test: `(5.0 + 0.5 + 2.5 + 2.79372) * 1.255 = 13.57` (approx) |
| M3.4 | Edge cases: negative spot prices, zero margin, DST transitions                                               | Unit tests for each edge case                                     |
| M3.5 | Implement `findCheapestWindow(prices: TotalPrice[], durationMinutes: number): Window`                        | Returns optimal start time and average price                      |
| M3.6 | Cheapest window edge cases: window longer than available prices, no future prices, exactly matching duration | Unit tests for each edge case                                     |
| M3.7 | Property: cheapest window result is provably optimal (no other window of same length has lower average)      | Test: brute-force verify against all possible windows             |

**Dependencies:** M2 complete (for price data), but calculation is pure — can develop in parallel with test data
**Estimated effort:** 1 session (~3 hours)
**Critical path:** Yes — this is the core product value

---

### M4: Auth & API Key System

**Goal:** Users can sign up, log in, and generate API keys for Home Assistant.

| Task | Detail                                                                | Acceptance                                           |
| ---- | --------------------------------------------------------------------- | ---------------------------------------------------- |
| M4.1 | Integrate Better Auth with Hono: email/password signup and login      | POST to auth endpoints creates user, returns session |
| M4.2 | Session middleware: protect routes that require authentication        | Unauthenticated requests get 401                     |
| M4.3 | API key generation endpoint: `POST /api/keys` → returns raw key once  | Key returned to user; hashed version stored in DB    |
| M4.4 | API key auth middleware: `Authorization: Bearer <key>` → resolve user | API calls with valid key return user-specific data   |
| M4.5 | API key management: list keys, delete key                             | User can see and revoke their keys                   |
| M4.6 | Rate limiting: basic per-key rate limit (e.g., 60 req/min)            | 429 returned when exceeded                           |

**Dependencies:** M1 complete (auth tables)
**Estimated effort:** 1 session (~3 hours)

---

### M5: REST API Endpoints

**Goal:** Two core API endpoints that Home Assistant can call.

| Task | Detail                                                                           | Acceptance                                                                   |
| ---- | -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| M5.1 | `GET /api/v1/price/now` — returns current total price with breakdown             | Returns JSON: `{total, spot, margin, transfer, tax, vat, hour, isNightRate}` |
| M5.2 | `GET /api/v1/price/cheapest?duration=180` — returns cheapest future window       | Returns JSON: `{start, end, averagePrice, prices: [...]}`                    |
| M5.3 | `GET /api/v1/price/today` — returns all hourly prices for today with total price | Returns array of 24 hourly price objects                                     |
| M5.4 | `GET /api/v1/price/tomorrow` — returns tomorrow's prices (if available)          | Returns array or `{available: false, expectedAt: "14:00 EET"}`               |
| M5.5 | Input validation and error responses (consistent JSON error format)              | Invalid duration returns 400 with clear message                              |
| M5.6 | Response time: all endpoints < 200ms under typical load                          | Verified with simple benchmark                                               |
| M5.7 | OpenAPI/JSON schema documentation for API endpoints                              | Machine-readable API docs                                                    |

**Dependencies:** M3 (calculation engine), M4 (API key auth)
**Estimated effort:** 1 session (~2–3 hours)

---

### M6: Web UI (Minimal)

**Goal:** Simple web interface for configuration and price visualization.

| Task | Detail                                                                     | Acceptance                                 |
| ---- | -------------------------------------------------------------------------- | ------------------------------------------ |
| M6.1 | Login/signup page                                                          | User can create account and log in         |
| M6.2 | Contract settings page: edit margin, transfer rates, tax, VAT, night hours | Settings saved to DB, validated            |
| M6.3 | Price chart: today's hourly total prices as a bar/line chart               | Visual chart renders with correct prices   |
| M6.4 | Tomorrow's prices display (when available, with "not yet available" state) | Shows prices or expected availability time |
| M6.5 | API key management page: generate, list, delete keys                       | User can create and manage keys            |
| M6.6 | Mobile-responsive layout                                                   | Usable on phone screen                     |

**Dependencies:** M4 (auth), M5 (API data)
**Technology decision:** Server-rendered HTML (Hono JSX or simple templates) vs. client-side SPA. **Recommendation: server-rendered** with Hono's built-in JSX for simplicity — no build step, no client framework, just HTML + minimal JS for the chart.
**Estimated effort:** 2 sessions (~4–5 hours)

---

### M7: Deployment & Operations

**Goal:** Running on Railway with automated deploys, health monitoring, and backups.

| Task | Detail                                                                   | Acceptance                                        |
| ---- | ------------------------------------------------------------------------ | ------------------------------------------------- |
| M7.1 | Railway project setup: service + volume (`/app/data`)                    | Service runs, volume mounted                      |
| M7.2 | Environment variables: `PORT`, `DATABASE_PATH`, `NODE_ENV`, auth secrets | App starts with Railway env vars                  |
| M7.3 | Health endpoint: `GET /health` checks DB connectivity                    | Returns 200 when healthy, 503 when DB unavailable |
| M7.4 | Graceful shutdown: SIGTERM handler closes DB connection                  | No corrupted WAL on deploy                        |
| M7.5 | Automated daily backups enabled on Railway volume                        | Backup schedule confirmed in Railway dashboard    |
| M7.6 | Deploy from `main` branch via Railway auto-deploy                        | Push to main triggers deploy                      |
| M7.7 | Smoke test after first deploy: prices fetched, API responds              | End-to-end verification                           |

**Dependencies:** M0–M6 complete (can start M7.1–M7.3 earlier in parallel)
**Estimated effort:** 1 session (~2 hours)

---

## Critical Path

```
M0 → M1 → M2 → M3 → M5 → M7
              ↘ M4 → M5 ↗
                  ↘ M6 ↗
```

- **M0 → M1 → M2 → M3 → M5** is the critical path (scaffolding → schema → prices → calculation → API)
- **M4** (auth) can start after M1, parallel with M2/M3
- **M6** (web UI) depends on M4 + M5, runs last before deployment
- **M7** (deployment) can start setup (M7.1–M7.3) after M0, full deploy after M6

## Dependency Graph

| Milestone | Depends on                 | Can parallel with |
| --------- | -------------------------- | ----------------- |
| M0        | —                          | —                 |
| M1        | M0                         | —                 |
| M2        | M1                         | M4                |
| M3        | M1 (for schema), test data | M2, M4            |
| M4        | M1                         | M2, M3            |
| M5        | M3, M4                     | —                 |
| M6        | M4, M5                     | M7 setup          |
| M7        | M6 (full), M0 (partial)    | M6                |

---

## Risks and Mitigations

| #   | Risk                                                  | Likelihood |  Impact  | Mitigation                                                                      |
| --- | ----------------------------------------------------- | :--------: | :------: | ------------------------------------------------------------------------------- |
| R1  | Nord Pool Data Portal API becomes restricted          |    Low     |   High   | Aggressive caching; if blocked, evaluate Elering/ENTSO-E (tested in research)   |
| R2  | better-sqlite3 native build fails on Railway          |    Low     |  Medium  | Test early in M0; prebuilt binaries available for Linux x64                     |
| R3  | Better Auth + Hono integration has rough edges        |   Medium   |   Low    | Better Auth is framework-agnostic; fallback: thin adapter layer                 |
| R4  | Day/night transfer rate logic gets timezone/DST wrong |   Medium   |   High   | Extensive unit tests with DST transition dates; use `date-fns-tz` or `Temporal` |
| R5  | Cheapest window algorithm is incorrect                |    Low     | Critical | Property-based testing: verify against brute-force for every test case          |
| R6  | Railway volume downtime during redeploy causes errors |    Low     |   Low    | Health check returns 503; clients retry                                         |

---

## Acceptance Criteria (MVP Launch)

| #   | Criterion                                                                 | Verification                                             |
| --- | ------------------------------------------------------------------------- | -------------------------------------------------------- |
| AC1 | User can sign up, log in, configure contract settings                     | Manual test: complete flow in < 5 minutes                |
| AC2 | Spot prices update daily without manual intervention                      | Automated: verify prices exist for today after 14:15 EET |
| AC3 | `GET /api/v1/price/now` returns correct total price                       | Unit test + manual verification against known price      |
| AC4 | `GET /api/v1/price/cheapest?duration=180` returns provably optimal window | Property test: brute-force comparison                    |
| AC5 | API responds < 200ms under typical load                                   | Benchmark test                                           |
| AC6 | API key auth works with Home Assistant REST integration                   | Manual test: configure HA sensor with API key            |
| AC7 | App runs on Railway for 7 consecutive days without unplanned downtime     | Monitoring: health check uptime                          |
| AC8 | All lint, typecheck, and test commands pass                               | CI pipeline green                                        |

---

## Quality Gate Checklist (Pre-Launch)

- [ ] All acceptance criteria (AC1–AC8) verified with evidence
- [ ] No `any` types in codebase
- [ ] No code duplication (DRY review)
- [ ] All pure functions have unit tests
- [ ] Price calculation tested with real Nord Pool data
- [ ] Cheapest window tested with edge cases (negative prices, DST, short/long windows)
- [ ] Secrets and API keys not committed to repository
- [ ] SIGTERM graceful shutdown tested
- [ ] Railway backup schedule confirmed
- [ ] README with setup instructions and API documentation

---

## Estimated Total Effort

| Milestone              | Sessions |   Hours (est.)   |
| ---------------------- | :------: | :--------------: |
| M0: Scaffolding        |    1     |       2–3        |
| M1: Schema             |    1     |        2         |
| M2: Price ingestion    |    1     |        3         |
| M3: Calculation engine |    1     |        3         |
| M4: Auth & API keys    |    1     |        3         |
| M5: REST API           |    1     |       2–3        |
| M6: Web UI             |    2     |       4–5        |
| M7: Deployment         |    1     |        2         |
| **Total**              |  **~9**  | **~21–25 hours** |

With parallel work on M2/M3/M4, the critical path is roughly **6–7 sessions**.

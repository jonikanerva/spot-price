# Spot Price

Nord Pool spot electricity price API with total price calculation and cheapest window finder. Built for Home Assistant and home automation integrations.

## What it does

Spot Price fetches day-ahead electricity prices from Nord Pool, combines them with your contract terms (margin, transfer fees, taxes), and exposes a REST API that returns:

- **Current total price** with full cost breakdown
- **Cheapest contiguous window** for scheduling flexible loads (EV charging, water heater, sauna, etc.)
- **Today's and tomorrow's prices** as quarter-hourly time series
- **Historical prices** for an inclusive local date range (up to 31 days)
- **A clearly-labelled FI price estimate** for the days Nord Pool has not published yet (derived from public Fingrid grid data — never presented as a published price)

Supports all 21 Nord Pool delivery areas across the Nordics, Baltics, and Central Europe.

## Tech stack

| Component  | Choice                                                               |
| ---------- | -------------------------------------------------------------------- |
| Runtime    | Node.js 24 LTS                                                       |
| Language   | TypeScript (strict mode)                                             |
| Framework  | [Hono](https://hono.dev) + @hono/node-server                         |
| Database   | PostgreSQL via the raw `pg` driver (no ORM; numbered SQL migrations) |
| Auth       | [Better Auth](https://better-auth.com) (self-hosted)                 |
| Scheduling | node-cron (in-process price fetch + hourly Fingrid grid-data fetch)  |
| Testing    | Vitest (unit) + Playwright (E2E)                                     |
| Hosting    | Railway                                                              |

## API endpoints

All `/api/v1/price/*` endpoints require an API key via `Authorization: Bearer <key>` header.

| Method | Endpoint                 | Description                                                                    |
| ------ | ------------------------ | ------------------------------------------------------------------------------ |
| GET    | `/api/v1/price/now`      | Current total price with breakdown                                             |
| GET    | `/api/v1/price/today`    | All of today's prices with total cost                                          |
| GET    | `/api/v1/price/tomorrow` | Tomorrow's prices (typically available after 12:00 UTC)                        |
| GET    | `/api/v1/price/all`      | Today's prices plus tomorrow's when published; each day's own `available` flag |
| GET    | `/api/v1/price/cheapest` | Cheapest contiguous window for a duration                                      |
| GET    | `/api/v1/price/history`  | Historical total prices for an inclusive local date range                      |
| GET    | `/api/v1/price/forecast` | FI-only price **estimate** for days Nord Pool has not published yet            |
| GET    | `/api/public/spot`       | Public spot prices (no auth required)                                          |
| GET    | `/health`                | Health check                                                                   |

The web UI also uses session-protected `/api/v1/me/*` routes (settings, chart); these are part of the setup UI, not the API-key surface.

### Authentication

`POST /api/session/login-or-signup` with `{username, password}` returns a session cookie. `GET /api/keys` (with that cookie) returns or creates the API key. Use it as `Authorization: Bearer <key>`.

The interactive API reference is served at `/api/docs` and the OpenAPI 3.1 document at `/api/v1/openapi.json`.

### Query parameters

**`/api/v1/price/cheapest`**

| Parameter   | Required | Description                                   |
| ----------- | -------- | --------------------------------------------- |
| `duration`  | Yes      | Window length in minutes (1-1440)             |
| `startTime` | No       | ISO 8601 start bound (default: now)           |
| `endTime`   | No       | ISO 8601 end bound (default: end of tomorrow) |
| `maxPrice`  | No       | Max allowed interval total (c/kWh, inclusive) |

**`/api/v1/price/history`**

| Parameter | Required | Description                                                             |
| --------- | -------- | ----------------------------------------------------------------------- |
| `from`    | Yes      | Inclusive range start, `YYYY-MM-DD`, interpreted in the user's timezone |
| `to`      | Yes      | Inclusive range end, `YYYY-MM-DD`, interpreted in the user's timezone   |

Totals apply your **current** contract settings to the stored historical spot prices (there is no historical settings versioning). The server enforces `from <= to` and a maximum inclusive span of 31 days; out-of-range requests return `400`. An empty-but-valid range returns `200` with `available: false` and an empty `prices` array.

**`/api/v1/price/forecast`**

| Parameter | Required | Description                                                                                    |
| --------- | -------- | ---------------------------------------------------------------------------------------------- |
| `area`    | No       | Delivery area (default: `FI`). The forecast is FI-only; other areas return `available: false`. |

The forecast is a **clearly-labelled estimate**, not a published price. Every money field is named `estimatedSpotCentsKwh` / `estimatedTotalCentsKwh`, each entry carries `estimated: true`, and the response carries `forecast: true` — so it shares no money-field name with the real-price endpoints and a misrouted consumer cannot read an estimate as a published price. Check `available`, `degraded`, and `confidence` (`normal` / `low`): the response degrades to low confidence rather than returning a confident guess when grid data is insufficient. The estimate is derived from public Fingrid grid data plus stored price history using simple closed-form math — there is no ML or price prediction.

**`/api/public/spot`**

| Parameter | Required | Description                         |
| --------- | -------- | ----------------------------------- |
| `area`    | No       | Nord Pool area code (default: `FI`) |

### Response types and interactive docs

Full request/response schemas with examples are available in the [Interactive API Documentation](https://spot.calmdonut.com/api/docs). The OpenAPI 3.1 spec is also available at [`/api/v1/openapi.json`](https://spot.calmdonut.com/api/v1/openapi.json).

### Error responses

All errors follow the same shape:

```json
{
  "error": "Description of what went wrong"
}
```

| Status | Meaning                                        |
| ------ | ---------------------------------------------- |
| 400    | Invalid request (bad parameters, invalid area) |
| 401    | Missing or invalid API key                     |
| 403    | Registration closed (user cap reached)         |
| 404    | No price data available                        |
| 429    | Rate limit exceeded                            |

### Rate limits

| Scope           | Limit             |
| --------------- | ----------------- |
| Global (per IP) | 120 requests/min  |
| API key         | 60 requests/min   |
| Login/signup    | 10 attempts/15min |

Standard `RateLimit-*` headers are included in responses (draft-6).

## Supported delivery areas

Finland (FI), Sweden (SE1-SE4), Norway (NO1-NO5), Denmark (DK1-DK2), Estonia (EE), Lithuania (LT), Latvia (LV), Austria (AT), Belgium (BE), France (FR), Germany (GER), Netherlands (NL), Poland (PL).

## Local development

### Prerequisites

This project requires [mise](https://mise.jdx.dev). `mise install` provisions the pinned Node 24.15.0 + pnpm 10 (and Python for the smoke script); `mise run setup` installs dependencies + Playwright browsers. The same versions are pinned in `.nvmrc` and `package.json` (`engines` / `packageManager`).

Docker is also required for the local PostgreSQL 17 used by `pnpm db:up`.

### Setup

```bash
git clone https://github.com/calmdonut/spot-price.git
cd spot-price
mise install      # provision Node 24 + pnpm 10 + Python (mise.toml [tools])
mise run setup    # install dependencies + Playwright browsers
pnpm db:up        # start local PostgreSQL 17 (waits until ready)
pnpm dev          # dev server → http://localhost:3000
```

`pnpm db:up` brings up the Docker Postgres and blocks until it is accepting
connections, so `pnpm dev`, `pnpm seed`, and the tests are non-racy. `pnpm db:down`
stops it (the data volume is kept).

### Available scripts

| Script              | Description                                                           |
| ------------------- | --------------------------------------------------------------------- |
| `pnpm dev`          | Start dev server with hot reload                                      |
| `pnpm build`        | Production build via tsup                                             |
| `pnpm start`        | Run production build                                                  |
| `pnpm test`         | Run unit tests (Vitest)                                               |
| `pnpm test:watch`   | Run unit tests in watch mode                                          |
| `pnpm test:e2e`     | Run E2E tests (Playwright) — needs `pnpm db:up`; see End-to-end tests |
| `pnpm test:all`     | Run all checks (typecheck + lint + test + build)                      |
| `pnpm typecheck`    | TypeScript type checking                                              |
| `pnpm lint`         | ESLint                                                                |
| `pnpm format`       | Prettier format (write)                                               |
| `pnpm format:check` | Prettier format check (no writes)                                     |
| `pnpm db:up`        | Start local PostgreSQL 17 via docker compose, waits for readiness     |
| `pnpm db:down`      | Stop local PostgreSQL (data volume kept)                              |
| `pnpm seed`         | Seed database with sample prices                                      |
| `pnpm backtest`     | Run the forecast backtest (fixture or `--db`) — see Backtesting       |
| `pnpm smoke`        | Run the local smoke-test script                                       |

### End-to-end tests

`pnpm test:e2e` runs Playwright against a local server backed by a dedicated `spot_price_e2e` database. Requires `pnpm db:up` first; each run resets the e2e database's schema (drop + recreate `public`, repopulated by the server's startup migration) and never touches your dev `spot_price` data — the e2e connection string is a hard-coded literal, and a `current_database()` guard aborts loudly if it ever points anywhere else.

### Verification & governance

There is no CI. Verification is manual: run `pnpm test:all` and `pnpm test:e2e` locally before pushing. Changes routed through the project's `/codereview` run `test:all` automatically; direct edits are verified by the author. (The repo hooks block destructive pushes and remind about `/codereview` — they do not run the tests.)

### Environment variables

| Variable              | Default | Description                                                                                                                                                                                                      |
| --------------------- | ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PORT`                | `3000`  | Server port                                                                                                                                                                                                      |
| `DATABASE_URL`        | —       | PostgreSQL connection string (required outside test mode)                                                                                                                                                        |
| `DATABASE_PUBLIC_URL` | —       | Optional. Used only by the offline backtest CLI (`pnpm backtest --db`) to reach the Railway DB from a dev machine. Never required at runtime — the server uses `DATABASE_URL`.                                   |
| `TEST_DATABASE_URL`   | —       | PostgreSQL connection string for tests (falls back to `DATABASE_URL`)                                                                                                                                            |
| `BETTER_AUTH_SECRET`  | —       | Auth secret (required in prod)                                                                                                                                                                                   |
| `BETTER_AUTH_URL`     | —       | Public URL (required in prod)                                                                                                                                                                                    |
| `FINGRID_API_KEY`     | —       | Fingrid Open Data API key for the FI forecast. Optional: when absent the forecast cron does not run and `/api/v1/price/forecast` returns `available: false`. A one-time startup warning is logged in production. |

## How it works

1. An in-process **cron** fetches day-ahead prices from the [Nord Pool Data Portal API](https://dataportal-api.nordpoolgroup.com) for all 21 delivery areas: every 2 hours as a baseline, plus a 10-minute burst during the publication window (12:00–13:59 CET) that pauses once tomorrow's prices are captured. A fetch also runs on startup.
2. Prices are stored in PostgreSQL as raw spot prices in EUR/MWh; Nord Pool's current granularity is 15-minute, and the calculator handles variable interval lengths.
3. When a user calls the API, their contract settings are applied: `total = (spot + margin + transfer + tax) * (1 + VAT%)`.
4. The cheapest window algorithm finds the optimal contiguous time slot using a weighted sliding window over variable-length intervals.
5. When `FINGRID_API_KEY` is set, a separate **hourly** cron fetches public Fingrid grid series (wind/consumption) into PostgreSQL. The FI forecast endpoint combines those with stored price history using simple closed-form math to produce a clearly-labelled estimate for days Nord Pool has not published yet. This path is fully isolated from the authoritative price crons — a Fingrid failure can never affect published prices, and the forecast degrades rather than guessing.

## Backtesting

The forecast's accuracy is measured on demand with an offline dev CLI under `tools/` (excluded from the production bundle; never runs in the server process). It replays the existing issue-time, leakage-guarded rolling-origin backtest and prints MAE / rMAE-vs-naive / sMAPE plus, per horizon (d+1/d+2/d+3), Spearman and precision@N (over the cheapest/peak 4-hour window) and empirical band coverage.

```
pnpm backtest --data <fixture.json> | --db [--window <days>] [--export <fixture.json>]
```

- `--data <fixture.json>` replays a committed fixture **file** (not a directory), e.g. `pnpm backtest --data tools/backtest-data/fixture.json`.
- `--db` scores against the live database over the last `--window` days (default 90). It connects via `DATABASE_PUBLIC_URL ?? DATABASE_URL`, so a local `--db` run needs `DATABASE_PUBLIC_URL` set to the Railway public Postgres URL (egress for one window is sub-cent).
- `--db --export <fixture.json>` snapshots the assembled window to a fixture; `--export X.json` ↔ `--data X.json` round-trips with no double-conversion.

Prediction bands ship **dark** (`calibrated: false`) — the forecast response carries no band/interval fields. To calibrate them, run `pnpm tsx tools/regenerate-bands.ts --data <fixture.json>` over accumulated real history; it rewrites `src/conformal-artifact.ts` only when the measured coverage gate clears. This is a periodic manual chore for a human to review and commit, not a background job; bands turn on purely by committing a calibrated artifact, no code change.

## Data source

Spot prices are sourced from the [Nord Pool Data Portal API](https://dataportal-api.nordpoolgroup.com/api) — the same API used by Home Assistant's official Nord Pool integration ([pynordpool](https://github.com/gjohansson-ST/pynordpool)). Data is owned by Nord Pool.

The FI price **estimate** additionally uses public grid data from [Fingrid Open Data](https://data.fingrid.fi) (the Finnish transmission system operator) — wind and consumption series only. This is the sole input to the forecast besides the stored Nord Pool price history; it is never blended into, or presented as, a published Nord Pool price. The forecast is a clearly-labelled estimate, FI only, and structurally separate from the real-price endpoints.

## License

Private project.

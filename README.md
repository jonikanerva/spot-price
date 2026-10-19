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

| Component  | Choice                                                                 |
| ---------- | ---------------------------------------------------------------------- |
| Language   | Rust 1.99 (edition 2024)                                               |
| Framework  | [Axum](https://docs.rs/axum) + Tokio                                   |
| Database   | PostgreSQL via [SQLx](https://docs.rs/sqlx) (checked queries, no ORM)  |
| Time       | [Jiff](https://docs.rs/jiff) with the bundled time zone database       |
| Auth       | Username and password (Better Auth-compatible scrypt hashes), API keys |
| Scheduling | In-process jobs (croner): Nord Pool, Fingrid, OpenWeatherMap           |
| Testing    | nextest, golden files from the former TypeScript service, Playwright   |
| Hosting    | Railway (Railpack, `railpack.json`)                                    |

`STACK.md` holds the full technical contract.

## API endpoints

All `/api/v1/price/*` endpoints require an API key via `Authorization: Bearer <key>` header.

| Method | Endpoint                 | Description                                                                            |
| ------ | ------------------------ | -------------------------------------------------------------------------------------- |
| GET    | `/api/v1/price/now`      | Current total price with breakdown                                                     |
| GET    | `/api/v1/price/today`    | All of today's prices with total cost                                                  |
| GET    | `/api/v1/price/tomorrow` | Tomorrow's prices (typically available after 12:00 UTC)                                |
| GET    | `/api/v1/price/all`      | All currently-known published prices in one list (today, plus tomorrow once published) |
| GET    | `/api/v1/price/cheapest` | Cheapest contiguous window for a duration                                              |
| GET    | `/api/v1/price/history`  | Historical total prices for an inclusive local date range                              |
| GET    | `/api/v1/price/forecast` | FI-only price **estimate** for days Nord Pool has not published yet                    |
| GET    | `/api/public/spot`       | Public spot prices (no auth required)                                                  |
| GET    | `/health`                | Health check                                                                           |

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

The service computes the forecast once after each data change (at start-up, after each price fetch that stores prices, and after each Fingrid fetch that stores data) and serves that result; your contract terms and time zone apply per request. `generatedAt` is the data freshness, not the request time: the later of the newest Fingrid forecast issuance (datasets 245 and 165) and the newest arrival of an FI price in the 30-day price history. If neither exists, it is the time of the computation.

**`/api/public/spot`**

| Parameter | Required | Description                         |
| --------- | -------- | ----------------------------------- |
| `area`    | No       | Nord Pool area code (default: `FI`) |

The response has the spot prices of today and tomorrow in the time zone of the area. `tomorrowAvailable` is `true` only when every quarter of tomorrow is stored. Before Nord Pool publishes the next day (about 12:00 UTC), `tomorrow` can already hold the first quarters of tomorrow, and `tomorrowAvailable` is `false`.

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

Install [mise](https://mise.jdx.dev) and [rustup](https://rustup.rs). `rust-toolchain.toml` pins Rust. `mise.toml` pins the other tools: Node and pnpm (Playwright and Prettier only), PostgreSQL 18.6, sqlx-cli, cargo-deny, cargo-nextest, cargo-shear, and Gitleaks. Docker is not needed.

### Setup

```bash
git clone https://github.com/jonikanerva/spot-price.git
cd spot-price
mise trust && mise run setup   # tools, locked dependencies, Playwright browser
mise run db:start              # isolated PostgreSQL 18.6 on 127.0.0.1:55436
```

Keep local settings in `.env.local`, not `.env`: the SQLx query macros read every `.env` file at compile time.

### Tasks

| Task                            | Description                                                                     |
| ------------------------------- | ------------------------------------------------------------------------------- |
| `mise run format`               | Format Rust and the other files (rustfmt, Prettier)                             |
| `mise run lint`                 | Clippy on every target; warnings are errors                                     |
| `mise run build`                | Release build of the server                                                     |
| `mise run test`                 | All Rust tests against the isolated database                                    |
| `mise run e2e`                  | Playwright journeys against the Rust binary                                     |
| `mise run verify`               | The full gate: format, lint, query metadata, contract, scans, build, tests, e2e |
| `mise run dev`                  | Migrate and run the service with `.env.local` and readable logs                 |
| `mise run sqlx:prepare`         | Regenerate `.sqlx` after a query or migration change                            |
| `mise run db:start` / `db:stop` | Start or stop the isolated test database                                        |

The server binary has three commands: `spot-price migrate`, `spot-price serve`, and `spot-price export-openapi <path>`.

### Verification & governance

There is no CI. Run `mise run verify` before pushing. A material change also gets an independent `/codereview`, and the reviewer runs `mise run verify` again on the integrated result. `STACK.md → Applicability and evidence` lists the full gate.

### Environment variables

| Variable                 | Default                 | Description                                                                                                |
| ------------------------ | ----------------------- | ---------------------------------------------------------------------------------------------------------- |
| `PORT`                   | `3000`                  | Server port                                                                                                |
| `DATABASE_URL`           | —                       | PostgreSQL connection string (required)                                                                    |
| `NODE_ENV`               | `development`           | `production` makes `BETTER_AUTH_URL` and `BETTER_AUTH_SECRET` required                                     |
| `BETTER_AUTH_URL`        | `http://localhost:3000` | Public origin. It sets the cookie security and the allowed `Origin` of session requests                    |
| `BETTER_AUTH_SECRET`     | —                       | Required in production. The Rust service does not use it; it stays for the revert path                     |
| `FINGRID_API_KEY`        | —                       | Fingrid Open Data key. Without it the Fingrid job does not run and the forecast returns `available: false` |
| `OPENWEATHERMAP_API_KEY` | —                       | OpenWeatherMap One Call 3.0 key. Without it the weather job does not run                                   |
| `LOG_FORMAT`             | `json`                  | `text` gives readable logs                                                                                 |
| `DATABASE_PUBLIC_URL`    | —                       | Only for `spot-price-tools --db` from a dev machine                                                        |

## How it works

1. An in-process **scheduled job** fetches day-ahead prices from the [Nord Pool Data Portal API](https://dataportal-api.nordpoolgroup.com) for all 21 delivery areas: every 2 hours as a baseline, plus a 10-minute burst during the publication window (12:00–13:59 CET) that pauses once tomorrow's prices are captured. A fetch also runs on startup.
2. Prices are stored in PostgreSQL as raw spot prices in EUR/MWh; Nord Pool's current granularity is 15-minute, and the calculator handles variable interval lengths.
3. When a user calls the API, their contract settings are applied: `total = (spot + margin + transfer + tax) * (1 + VAT%)`.
4. The cheapest window algorithm finds the optimal contiguous time slot using a weighted sliding window over variable-length intervals.
5. When `FINGRID_API_KEY` is set, a separate **hourly** job fetches public Fingrid grid series (wind/consumption) into PostgreSQL. The FI forecast endpoint combines those with stored price history using simple closed-form math to produce a clearly-labelled estimate for days Nord Pool has not published yet. This path is fully isolated from the authoritative price job — a Fingrid failure can never affect published prices, and the forecast degrades rather than guessing.

## Backtesting

The forecast's accuracy is measured on demand with the offline `spot-price-tools` binary (`crates/tools`). The release build does not include it, and it never runs in the server process. It replays the issue-time, leakage-guarded rolling-origin backtest and prints MAE / rMAE-vs-naive / sMAPE plus, per horizon (d+1/d+2/d+3), Spearman and precision@N and empirical band coverage.

```bash
cargo run -p spot-price-tools -- backtest --data crates/tools/data/fixture.json
cargo run -p spot-price-tools -- backtest --db [--window <days>] [--export <file>] [--compare]
cargo run -p spot-price-tools -- backtest --db [--window <days>] --ab <key=value[,key=value]>
cargo run -p spot-price-tools -- revision-magnitude --db [--window <days>]
cargo run -p spot-price-tools -- regenerate-bands --data <file>
```

- `--db` reads `DATABASE_PUBLIC_URL` (or `--db-url`) from the process environment. No `.env` file is read.
- `--db --export <file>` snapshots the assembled window; `--data <file>` replays it.
- `--ab` scores the production model (A) and a variant (B) on the same origins. It also works with `--data`, but not with `--compare`. The variant keys are `ridge_lambda`, `wind_extension_weeks`, `consumption_extension_weeks`, `latest_published` (`before` is production, `at_or_before` is the variant), and `prev_day_basis` (`utc` is production; `delivery` reads the previous CET/CEST delivery day). The paired MAE per horizon is the verdict line.

Prediction bands ship **dark** (`calibrated: false`). `regenerate-bands` rewrites `crates/domain/src/forecast/conformal_artifact.rs` only when the coverage gate clears. A human reviews and commits the result; bands turn on by committing a calibrated artifact.

## Data source

Spot prices are sourced from the [Nord Pool Data Portal API](https://dataportal-api.nordpoolgroup.com/api) — the same API used by Home Assistant's official Nord Pool integration ([pynordpool](https://github.com/gjohansson-ST/pynordpool)). Data is owned by Nord Pool.

The FI price **estimate** additionally uses public grid data from [Fingrid Open Data](https://data.fingrid.fi) (the Finnish transmission system operator) — wind and consumption series only. This is the sole input to the forecast besides the stored Nord Pool price history; it is never blended into, or presented as, a published Nord Pool price. The forecast is a clearly-labelled estimate, FI only, and structurally separate from the real-price endpoints.

## License

Private project.

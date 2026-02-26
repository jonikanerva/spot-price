# Spot Price

Nord Pool spot electricity price API with total price calculation and cheapest window finder. Built for Home Assistant and home automation integrations.

## What it does

Spot Price fetches day-ahead electricity prices from Nord Pool, combines them with your contract terms (margin, transfer fees, taxes), and exposes a REST API that returns:

- **Current total price** with full cost breakdown
- **Cheapest contiguous window** for scheduling flexible loads (EV charging, water heater, sauna, etc.)
- **Today's and tomorrow's prices** as hourly/quarter-hourly time series

Supports all 21 Nord Pool delivery areas across the Nordics, Baltics, and Central Europe.

## Tech stack

| Component  | Choice                                               |
| ---------- | ---------------------------------------------------- |
| Runtime    | Node.js 22 LTS                                       |
| Language   | TypeScript (strict mode)                             |
| Framework  | [Hono](https://hono.dev) + @hono/node-server         |
| Database   | SQLite via better-sqlite3 (WAL mode)                 |
| Auth       | [Better Auth](https://better-auth.com) (self-hosted) |
| Scheduling | node-cron (in-process daily price fetch)             |
| Testing    | Vitest (unit) + Playwright (E2E)                     |
| Hosting    | Railway                                              |

## API endpoints

All `/api/v1/price/*` endpoints require an API key via `Authorization: Bearer <key>` header.

| Method | Endpoint                 | Description                                    |
| ------ | ------------------------ | ---------------------------------------------- |
| GET    | `/api/v1/price/now`      | Current total price with breakdown             |
| GET    | `/api/v1/price/today`    | All of today's prices with total cost          |
| GET    | `/api/v1/price/tomorrow` | Tomorrow's prices (available after ~14:00 EET) |
| GET    | `/api/v1/price/cheapest` | Cheapest contiguous window for a duration      |
| GET    | `/api/public/spot`       | Public spot prices (no auth required)          |
| GET    | `/health`                | Health check                                   |

### Query parameters

**`/api/v1/price/cheapest`**

| Parameter   | Required | Description                                   |
| ----------- | -------- | --------------------------------------------- |
| `duration`  | Yes      | Window length in minutes (1-1440)             |
| `startTime` | No       | ISO 8601 start bound (default: now)           |
| `endTime`   | No       | ISO 8601 end bound (default: end of tomorrow) |

**`/api/public/spot`**

| Parameter | Required | Description                         |
| --------- | -------- | ----------------------------------- |
| `area`    | No       | Nord Pool area code (default: `FI`) |

### Response types

#### `GET /api/v1/price/now` — TotalPrice

```json
{
  "deliveryStart": "2026-02-26T10:00:00Z",
  "deliveryEnd": "2026-02-26T10:15:00Z",
  "localStart": "2026-02-26T12:00:00+02:00",
  "localEnd": "2026-02-26T12:15:00+02:00",
  "spotCentsKwh": 5.23,
  "marginCentsKwh": 0.5,
  "transferCentsKwh": 2.5,
  "taxCentsKwh": 2.79372,
  "vatCentsKwh": 2.762,
  "totalCentsKwh": 13.786,
  "isNightRate": false
}
```

#### `GET /api/v1/price/today` — Price list

```json
{
  "prices": [
    {
      "deliveryStart": "...",
      "deliveryEnd": "...",
      "localStart": "...",
      "localEnd": "...",
      "spotCentsKwh": 5.23,
      "marginCentsKwh": 0.5,
      "transferCentsKwh": 2.5,
      "taxCentsKwh": 2.79372,
      "vatCentsKwh": 2.762,
      "totalCentsKwh": 13.786,
      "isNightRate": false
    }
  ],
  "available": true
}
```

#### `GET /api/v1/price/tomorrow` — Price list (or unavailable)

```json
{
  "prices": [],
  "available": false,
  "expectedAt": "14:00 EET"
}
```

#### `GET /api/v1/price/cheapest` — Cheapest window

```json
{
  "start": "2026-02-26T01:00:00Z",
  "end": "2026-02-26T04:00:00Z",
  "startLocal": "2026-02-26T03:00:00+02:00",
  "endLocal": "2026-02-26T06:00:00+02:00",
  "averageTotalCentsKwh": 8.452,
  "prices": [
    { "...TotalPrice objects..." }
  ]
}
```

#### `GET /api/public/spot` — Public spot prices

```json
{
  "area": "FI",
  "today": [
    {
      "deliveryStart": "2026-02-25T22:00:00Z",
      "deliveryEnd": "2026-02-25T22:15:00Z",
      "priceEurMwh": 52.3,
      "area": "FI",
      "spotCentsKwh": 5.23
    }
  ],
  "tomorrow": [],
  "tomorrowAvailable": false,
  "unit": "c/kWh",
  "resolutionMinutes": 15
}
```

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

- Node.js >= 22
- npm

### Setup

```bash
git clone https://github.com/calmdonut/spot-price.git
cd spot-price
npm install
```

### Run development server

```bash
npm run dev
```

Server starts at `http://localhost:3000`.

### Available scripts

| Script              | Description                      |
| ------------------- | -------------------------------- |
| `npm run dev`       | Start dev server with hot reload |
| `npm run build`     | Production build via tsup        |
| `npm start`         | Run production build             |
| `npm test`          | Run unit tests (Vitest)          |
| `npm run test:e2e`  | Run E2E tests (Playwright)       |
| `npm run typecheck` | TypeScript type checking         |
| `npm run lint`      | ESLint                           |
| `npm run format`    | Prettier format                  |
| `npm run seed`      | Seed database with sample prices |

### Environment variables

| Variable             | Default        | Description                    |
| -------------------- | -------------- | ------------------------------ |
| `PORT`               | `3000`         | Server port                    |
| `DATABASE_PATH`      | `data/spot.db` | SQLite database file path      |
| `BETTER_AUTH_SECRET` | —              | Auth secret (required in prod) |
| `BETTER_AUTH_URL`    | —              | Public URL (required in prod)  |

## How it works

1. **Daily cron** fetches day-ahead prices from the [Nord Pool Data Portal API](https://data.nordpoolgroup.com) for all 21 delivery areas at ~12:00 UTC.
2. Prices are stored in SQLite (15-minute resolution, EUR/MWh).
3. When a user calls the API, their contract settings are applied: `total = (spot + margin + transfer + tax) * (1 + VAT%)`.
4. The cheapest window algorithm finds the optimal contiguous time slot using a weighted sliding window over variable-length intervals.

## Data source

Spot prices are sourced from the [Nord Pool Data Portal API](https://dataportal-api.nordpoolgroup.com/api) — the same API used by Home Assistant's official Nord Pool integration. Data is owned by Nord Pool.

## License

Private project.

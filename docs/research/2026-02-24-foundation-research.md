# Research Dossier: Spot-Price Foundation

Date: 2026-02-24
Author: Research Agent
Status: **Approved** — owner GO on 2026-02-24

---

## Problem Statement

Finnish households on spot-priced electricity contracts need to know:

1. The current and upcoming hourly electricity price (including VAT, transfer fees, and margin).
2. The cheapest contiguous N-hour window for scheduling flexible loads (EV charging, water heater, sauna, etc.).

Existing solutions (porssisahko.net, sahkotin.fi) show prices but don't offer a programmable API that calculates **total price** (spot + transfer + tax + margin) or finds **optimal scheduling windows** for home automation systems like Home Assistant.

## Target Users

| User                            | Job-to-be-done                                                   |
| ------------------------------- | ---------------------------------------------------------------- |
| Home Assistant enthusiast       | Automate appliances to run during cheapest hours via REST API    |
| Spot-price electricity customer | See today's/tomorrow's total price at a glance                   |
| Developer building integrations | Consume a clean JSON API for Finnish spot prices with total cost |

## Constraints

- **Geography**: Finland only (Nordpool FI bidding area, EIC code 10YFI-1--------U).
- **Budget**: Minimal; hobby-tier hosting (~€5–10/month).
- **Team**: Solo developer + AI agent.
- **Data licensing**: Nord Pool owns spot price data. Redistribution must comply with their terms.
- **Regulatory**: Finnish VAT on electricity is 25.5% (as of 2024; was temporarily 10% in 2023).

---

## Topic 1: Spot Price Data Sources for Finland

### Source A: sahkotin.fi (Pakastin Oy)

**Verified live** — API tested successfully on 2026-02-24.

| Attribute                 | Detail                                                                             |
| ------------------------- | ---------------------------------------------------------------------------------- |
| **Endpoint**              | `GET https://sahkotin.fi/prices?fix&start=<ISO>&end=<ISO>`                         |
| **Auth**                  | None required                                                                      |
| **Rate limits**           | "Normal queries allowed; excessive load is not" (no hard number)                   |
| **Data format**           | JSON: `{"prices":[{"date":"ISO","value":12.4},...]}`                               |
| **Unit**                  | Default: EUR/MWh. With `&fix` parameter: c/kWh. With `&vat`: includes Finnish VAT. |
| **Quarter-hour**          | Supported via `&quarter` parameter (15-min resolution)                             |
| **Historical depth**      | Data available from at least 2023-01-01 onward                                     |
| **Next-day availability** | Prices appear after Nord Pool publishes (~12:45 CET / 13:45 EET)                   |
| **CSV support**           | Yes, via `/prices.csv` endpoint                                                    |
| **Terms**                 | Free for **non-commercial** use. Data owned by Nord Pool.                          |
| **Reliability**           | Single small Finnish company (Pakastin Oy). No SLA.                                |

**Evidence**: Live response for 2026-02-24 returned 24 hourly prices in c/kWh with `&fix` parameter. Values ranged 8.8–18.3 c/kWh. [Source: direct API call]

### Source B: api.porssisahko.net

**Verified live** — API tested successfully on 2026-02-24.

| Attribute                 | Detail                                                                      |
| ------------------------- | --------------------------------------------------------------------------- |
| **Endpoint**              | `GET https://api.porssisahko.net/v1/latest-prices.json`                     |
| **Auth**                  | None required                                                               |
| **Rate limits**           | Undocumented; community-run                                                 |
| **Data format**           | JSON: `{"prices":[{"price":13.387,"startDate":"ISO","endDate":"ISO"},...]}` |
| **Unit**                  | c/kWh (includes Finnish VAT)                                                |
| **Historical depth**      | Returns ~48 hours (today + tomorrow when available)                         |
| **Next-day availability** | Same timing as sahkotin.fi (~13:45 EET)                                     |
| **Terms**                 | Undocumented. Widely used by Finnish HA community.                          |
| **Reliability**           | Community project. No SLA. Has been stable for years.                       |

**Evidence**: Live response returned 48 price entries spanning 2026-02-23 to 2026-02-24, prices in c/kWh with VAT. [Source: direct API call]

### Source C: Elering API (Estonian TSO)

**Verified live** — API tested successfully on 2026-02-24.

| Attribute                 | Detail                                                                         |
| ------------------------- | ------------------------------------------------------------------------------ |
| **Endpoint**              | `GET https://dashboard.elering.ee/api/nps/price?start=<ISO>&end=<ISO>`         |
| **Auth**                  | None required                                                                  |
| **Rate limits**           | Undocumented; public API                                                       |
| **Data format**           | JSON: `{"success":true,"data":{"fi":[{"timestamp":unix,"price":110.22},...]}}` |
| **Unit**                  | EUR/MWh                                                                        |
| **Countries**             | Estonia (ee), Finland (fi), Latvia (lv), Lithuania (lt) — all in one response  |
| **Quarter-hour**          | Yes — response contained 15-min intervals (96 entries/day)                     |
| **Historical depth**      | At least back to 2024-01-01 (tested)                                           |
| **Next-day availability** | Same timing as Nord Pool publication                                           |
| **Terms**                 | Public API from Estonian TSO. No explicit restriction found.                   |
| **Reliability**           | Government-backed TSO. More reliable than community projects.                  |

**Evidence**: Tested with `start=2026-02-24&end=2026-02-25` — returned 96 quarter-hourly entries for Finland (`fi` key) in EUR/MWh. Also tested historical data for 2024-01-01 successfully. [Source: direct API calls]

### Source D: ENTSO-E Transparency Platform

| Attribute                 | Detail                                                                      |
| ------------------------- | --------------------------------------------------------------------------- |
| **Endpoint**              | `https://web-api.tp.entsoe.eu/api` (RESTful XML API)                        |
| **Auth**                  | **Required** — free registration, API security token needed                 |
| **Rate limits**           | 400 requests per minute per token                                           |
| **Data format**           | XML (complex, verbose)                                                      |
| **Unit**                  | EUR/MWh                                                                     |
| **Historical depth**      | Years of data (official EU transparency regulation data)                    |
| **Next-day availability** | Official source; prices published ~12:45 CET                                |
| **Terms**                 | Free for non-commercial and commercial use under ACER regulation            |
| **Reliability**           | **Highest** — official EU-mandated platform                                 |
| **Complexity**            | High — XML parsing, area codes (Finland = 10YFI-1--------U), document types |

**Evidence**: ENTSO-E API guide confirms authentication requirement and XML format. Could not test directly (registration required). [Source: ENTSO-E web API documentation]

### Source E: Nord Pool Data Portal API (backend for data.nordpoolgroup.com)

**Confirmed in use** — this is the API that Home Assistant's official `nordpool` integration (platinum quality scale) calls via the `pynordpool` library (v0.3.2).

| Attribute                 | Detail                                                                                                                                                                                           |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Base URL**              | `https://dataportal-api.nordpoolgroup.com/api`                                                                                                                                                   |
| **Key endpoint**          | `GET /DayAheadPrices?date=<YYYY-MM-DD>&market=DayAhead&deliveryArea=<areas>&currency=<cur>`                                                                                                      |
| **Auth**                  | **None required** — no OAuth2, no API key, no headers                                                                                                                                            |
| **Rate limits**           | Undocumented; used by all Home Assistant Nord Pool installations worldwide                                                                                                                       |
| **Data format**           | JSON                                                                                                                                                                                             |
| **Unit**                  | EUR/MWh (or requested currency via `currency` param: EUR, SEK, NOK, DKK, PLN, BGN, RON)                                                                                                          |
| **Price status**          | `areaStates[].state` = `"Final"` or `"Preliminary"` — **unique among free sources**                                                                                                              |
| **Exchange rate**         | Included in response (`exchangeRate` field)                                                                                                                                                      |
| **Block prices**          | Included (`blockPriceAggregates[]` with named time blocks and area averages)                                                                                                                     |
| **Area averages**         | Included (`areaAverages[]` per area)                                                                                                                                                             |
| **Supported areas**       | All Nord Pool areas: FI, SE1-4, NO1-5, DK1-2, EE, LT, LV, AT, BE, FR, GER, NL, PL, BG, TEL, SYS                                                                                                  |
| **Historical depth**      | At least days; exact depth not tested yet                                                                                                                                                        |
| **Next-day availability** | Same timing as Nord Pool publication (~12:42 CET / 13:42 EET)                                                                                                                                    |
| **Retry behavior**        | pynordpool retries 3× with 7s delay on failure                                                                                                                                                   |
| **Terms**                 | Not explicitly documented for this API. It backs the public Data Portal website (`data.nordpoolgroup.com`). Used by Home Assistant's official platinum-tier integration without reported issues. |
| **Reliability**           | **Highest** — Nord Pool's own infrastructure; same backend as their public data portal                                                                                                           |
| **Complexity**            | **Low** — simple GET with query params, clean JSON response, no auth flow                                                                                                                        |

**Response structure:**

```json
{
  "deliveryDateCET": "2026-02-24",
  "updatedAt": "2026-02-24T12:45:00Z",
  "currency": "EUR",
  "exchangeRate": 1.0,
  "multiAreaEntries": [
    {
      "deliveryStart": "2026-02-24T00:00:00+01:00",
      "deliveryEnd": "2026-02-24T01:00:00+01:00",
      "entryPerArea": { "FI": 45.23 }
    }
  ],
  "areaStates": [{ "state": "Final", "areas": ["FI"] }],
  "areaAverages": [{ "areaCode": "FI", "price": 52.1 }],
  "blockPriceAggregates": [
    {
      "blockName": "Off-peak 1",
      "deliveryStart": "...",
      "deliveryEnd": "...",
      "averagePricePerArea": { "FI": { "average": 38.5 } }
    }
  ]
}
```

**How Home Assistant uses it:**

The `pynordpool` library (by @gjohansson-ST, MIT licensed) is the official client:

1. Creates `NordPoolClient(session)` — no credentials passed
2. Calls `async_get_delivery_period(date, Currency.EUR, ["FI"])` for yesterday, today, and tomorrow
3. Parses `multiAreaEntries` into hourly price entries with start/end times
4. Checks `areaStates[].state == "Final"` to determine if prices are finalized
5. Updates every hour; listener updates every 15 minutes
6. Retries 3× with 7-second delays on failure

**Evidence**: Full source code analyzed from `pynordpool` v0.3.2 (`pynordpool/__init__.py`, `pynordpool/const.py`, `pynordpool/model.py`) and Home Assistant core `homeassistant/components/nordpool/` (coordinator.py, config_flow.py, **init**.py). The base URL `https://dataportal-api.nordpoolgroup.com/api` is hardcoded in `pynordpool/const.py`. No authentication is sent in any request. [Source: github.com/gjohansson-ST/pynordpool, github.com/home-assistant/core/tree/dev/homeassistant/components/nordpool]

### Source F: Nord Pool Official Market Data API (OAuth2-protected)

**Not tested live** — requires customer onboarding.

| Attribute              | Detail                                                    |
| ---------------------- | --------------------------------------------------------- |
| **Developer portal**   | `https://developers.nordpoolgroup.com`                    |
| **Base URL**           | `https://data-api.nordpoolgroup.com/api/v2/`              |
| **Key endpoint**       | `GET /api/v2/Auction/Prices/ByAreas`                      |
| **Auth**               | **Required** — OAuth2 (client credentials → bearer token) |
| **Access requirement** | Must be a Nord Pool customer or request access via form   |
| **Data format**        | JSON (REST v2)                                            |
| **Reliability**        | Highest — official, with SLA for customers                |

This is the **formal, OAuth2-protected** Market Data API aimed at trading companies and ISV partners. It provides the same underlying data as the Data Portal API (Source E) but with documented SLAs, rate limits, and contractual terms.

**Key distinction from Source E**: Source E (Data Portal API) is the **open backend** of Nord Pool's public website. Source F is the **commercial API product** with OAuth2, rate limit documentation, and formal customer agreements. For our use case, Source E is sufficient and far simpler.

**Evidence**: Developer portal structure confirmed. API General Terms effective 16.12.2023. [Source: developers.nordpoolgroup.com, nordpoolgroup.com/en/trading/api/]

### Source G: spot-hinta.fi

| Attribute       | Detail                                                                                          |
| --------------- | ----------------------------------------------------------------------------------------------- |
| **Endpoint**    | `https://api.spot-hinta.fi/`                                                                    |
| **Auth**        | None                                                                                            |
| **Data format** | JSON                                                                                            |
| **Unit**        | c/kWh                                                                                           |
| **Notes**       | Another Finnish community API. Used by some HA integrations. Less documented than alternatives. |

**Not tested live** — included for completeness.

### Comparison Matrix

| Criterion             |   sahkotin.fi    | porssisahko.net |   Elering    |    ENTSO-E     | Nord Pool Data Portal  | Nord Pool Market Data API |
| --------------------- | :--------------: | :-------------: | :----------: | :------------: | :--------------------: | :-----------------------: |
| No auth needed        |        ✅        |       ✅        |      ✅      |       ❌       |           ✅           |            ❌             |
| Open access           |        ✅        |       ✅        |      ✅      | ✅ (free reg.) |     ✅ (de facto)      |    ❌ (customer only)     |
| Finnish prices        |        ✅        |       ✅        |      ✅      |       ✅       |           ✅           |            ✅             |
| c/kWh native          | ✅ (with `&fix`) |       ✅        | ❌ (EUR/MWh) |  ❌ (EUR/MWh)  |      ❌ (EUR/MWh)      |       ❌ (EUR/MWh)        |
| VAT included option   | ✅ (with `&vat`) |   ✅ (always)   |      ❌      |       ❌       |           ❌           |            ❌             |
| Quarter-hour data     |        ✅        |       ❌        |      ✅      |       ✅       |      ❌ (hourly)       |            ✅             |
| Historical depth      |      Years       |      ~48h       |    Years     |     Years      |         Days+          |           Years           |
| JSON format           |        ✅        |       ✅        |      ✅      |    ❌ (XML)    |           ✅           |            ✅             |
| Reliability           | Low (1 company)  | Low (community) | Medium (TSO) |   High (EU)    |  **Highest** (source)  |   **Highest** (source)    |
| Commercial use OK     |        ❌        |     Unknown     |  Likely yes  |       ✅       | Unknown (de facto yes) |     ✅ (if customer)      |
| Price status tracking |        ❌        |       ❌        |      ❌      |       ❌       | ✅ (Preliminary/Final) |            ✅             |
| Used by HA official   |        ❌        |       ❌        |      ❌      |       ❌       |   ✅ (platinum tier)   |            ❌             |

### Recommendation: Data Sources

**Single source: Nord Pool Data Portal API** (based on pynordpool/HA source code analysis)

- Authoritative source: data comes directly from Nord Pool's own infrastructure
- No authentication required — simple GET requests with query parameters
- Clean JSON response with hourly prices per area, price status (Preliminary/Final), exchange rates, block prices
- Battle-tested: used by Home Assistant's official `nordpool` integration (platinum quality scale) via `pynordpool` library — every HA Nord Pool installation worldwide calls this API
- Requires EUR/MWh → c/kWh conversion (÷10) and manual VAT addition — trivial math
- Risk: no formal SLA or documented terms for programmatic use; Nord Pool could restrict access
- Mitigation: low polling frequency (2–3×/day) and aggressive caching minimize abuse risk

**No fallback sources.** If the Data Portal API becomes unavailable, the service fails openly. Rationale:

- This is a hobby project for tens of users, not a critical production system
- Maintaining multiple data source integrations adds complexity without proportional value
- The Data Portal API is used by HA's platinum-tier integration with no reported outages — it's reliable enough
- If Nord Pool ever closes this API, re-evaluating data sources at that point is more practical than maintaining unused fallback code now

### Update Timing (Critical for Scheduling)

Nord Pool publishes next-day prices at approximately **12:42 CET (13:42 EET)**. All downstream APIs reflect this within minutes. The daily cron job should run at **~14:00 EET** to reliably capture tomorrow's prices, with a retry at **~15:00 EET** as fallback.

---

## Topic 2: Tech Stack Choices

### Web Framework

| Framework   |      TS Support       |      Performance      |         Ecosystem          | Bundle Size |        Node.js Native         |
| ----------- | :-------------------: | :-------------------: | :------------------------: | :---------: | :---------------------------: |
| **Hono**    |      First-class      |       Ultrafast       | Growing (2100+ dependents) |    ~14kB    | Via @hono/node-server adapter |
| **Fastify** | Good (built-in types) | Very fast (30k req/s) |    Mature (308 plugins)    |   Larger    |            Native             |
| **Express** |      Via @types       |       Moderate        |          Massive           |  Moderate   |            Native             |

**Recommendation: Hono**

Rationale:

- First-class TypeScript with type-safe routes and middleware
- 20M weekly npm downloads (as of Feb 2026) — massive adoption
- Ultralight (~14kB), perfect for small monolith
- Built-in middleware: CORS, JWT, basic auth, logger, cookie
- Clean, modern API (`c.json()`, `c.text()`, `c.html()`)
- Works on Node.js via `@hono/node-server` (verified: Node 18.14.1+)
- If we ever want to move to Cloudflare Workers or Bun, zero code changes
- MIT licensed

Trade-off vs Fastify: Fastify has a more mature plugin ecosystem and built-in schema validation (JSON Schema → serialization). But for a small monolith with <10 routes, Hono's simplicity wins. Fastify's plugin system adds complexity we don't need.

Trade-off vs Express: Express is legacy at this point. No built-in TypeScript, callback-based patterns, slower performance. No reason to choose it for a new project.

**Evidence**: Hono npm page shows v4.12.2, 20M weekly downloads, MIT license. Node.js adapter documented at hono.dev/docs/getting-started/nodejs. [Source: npmjs.com/package/hono, hono.dev]

### SQLite Library

| Library                          |  Sync API  | WAL Mode |       Migrations       |  ORM Features  |  Native Addon  |
| -------------------------------- | :--------: | :------: | :--------------------: | :------------: | :------------: |
| **better-sqlite3**               |     ✅     |    ✅    |         Manual         | None (raw SQL) |   Yes (C++)    |
| **drizzle-orm + better-sqlite3** |     ✅     |    ✅    | Built-in (drizzle-kit) |    Full ORM    | Yes (via b-s3) |
| **Turso/libsql**                 | ❌ (async) |   N/A    |      Via drizzle       |    Full ORM    |    Optional    |

**Recommendation: better-sqlite3 (direct, no ORM)**

Rationale:

- Synchronous API is actually **better** for SQLite concurrency (avoids mutex thrashing)
- 7k GitHub stars, 176k dependents, battle-tested
- WAL mode: single `db.pragma('journal_mode = WAL')` call
- Performance: 2–24x faster than async alternatives (per their own benchmarks)
- For a schema with ~3 tables (prices, users, api_keys), raw SQL is simpler than an ORM
- Migrations: simple SQL files executed on startup (no framework needed)
- Prebuilt binaries available for common platforms

Trade-off vs Drizzle ORM: Drizzle adds type-safe queries, automatic migrations via drizzle-kit, and a nice DX. But for 3 tables and ~10 queries, it's overhead. The schema is simple enough that hand-written SQL with TypeScript interfaces is sufficient. Drizzle can be added later if the schema grows.

Trade-off vs Turso: Turso is a hosted SQLite service (libsql). It solves the "SQLite on ephemeral infrastructure" problem but adds a network hop, a dependency on an external service, and cost. For a Railway volume-backed SQLite, it's unnecessary complexity.

**Evidence**: better-sqlite3 GitHub shows v12.6.2 (Jan 2026), 7k stars, MIT license. WAL mode documented in their performance guide. [Source: github.com/WiseLibs/better-sqlite3]

### Authentication

| Approach                       |       Type       | SQLite Support | TS Support  | Weekly Downloads |          Cost           | API Keys Built-in |
| ------------------------------ | :--------------: | :------------: | :---------: | :--------------: | :---------------------: | :---------------: |
| **Better Auth**                | Self-hosted lib  |  ✅ (native)   | First-class |       1.2M       |       Free (MIT)        |    Via plugin     |
| **Auth0**                      |  SaaS (hosted)   |      N/A       |    Good     |       N/A        | Free < 25k MAU, $35/mo+ |   Custom needed   |
| **Lucia v3**                   | Lib (deprecated) |       ✅       |    Good     | 100k (declining) |       Free (MIT)        |        No         |
| **Passport.js**                |    Middleware    |     Manual     | Via @types  |       1.1M       |       Free (MIT)        |        No         |
| **Custom (bcrypt + sessions)** |       DIY        |       ✅       |   Manual    |       N/A        |          Free           |      Manual       |

**Recommendation: Better Auth**

Rationale:

- **Suosittu ja testattu**: 1.2M viikkolatausta npm:stä, 317 dependents, aktiivinen kehitys (v1.4.19, päivitetty helmikuu 2026)
- **Framework-agnostic**: toimii Honon kanssa suoraan — ei vaadi Express:iä tai Next.js:ää
- **SQLite-tuki sisäänrakennettu**: toimii better-sqlite3:n kanssa natiivisti, ei tarvitse erillistä tietokantapalvelinta
- **TypeScript-first**: kirjoitettu kokonaan TypeScriptillä, tyyppiturvallinen API
- **Kattavat ominaisuudet valmiina**: email/password, sessiot, CSRF-suojaus, salasanan hashaus — ei tarvitse keksiä pyörää uudelleen
- **Plugin-ekosysteemi**: API key -tuki pluginin kautta, 2FA, passkeys, social login jne. saatavilla myöhemmin
- **Self-hosted**: ei ulkoista riippuvuutta, ei kuukausimaksua, data pysyy omassa SQLite-kannassa
- **MCP-tuki**: tarjoaa MCP-serverin AI-avusteiseen kehitykseen (hyödyllistä tässä projektissa)

Trade-off vs Auth0:

- Auth0 on SaaS-palvelu (Okta) — korkea laatu, mutta lisää ulkoisen riippuvuuden, verkkoviiveen, ja kustannuksia ($35/kk Essentials). Free-taso riittäisi (25k MAU), mutta hobby-projektille palvelun ylläpito on turhaa monimutkaisuutta. API key -tukea ei ole sisäänrakennettuna — pitäisi rakentaa erikseen.
- Auth0 on järkevä valinta SaaS-tuotteelle, ei pienen hobby-API:n autentikaatioon.

Trade-off vs Lucia:

- Lucia v3 npm-paketti on **virallisesti deprecated**. Kirjoittaja (pilcrowOnPaper) on siirtynyt tekemään Luciasta opetusresurssin, ei kirjastoa. Ei kannata rakentaa deprecated-riippuvuudelle.

Trade-off vs Passport.js:

- Vanha arkkitehtuuri (callback-pohjainen), Express-painotteinen, ei natiivi TypeScript-tukea. Ei syytä valita uuteen projektiin.

Trade-off vs Custom:

- ~100 riviä koodia vs. testattu kirjasto jota 1.2M lataa viikossa. Custom-koodi vaatii session-hallinnan, CSRF:n, salasanan hasauksen ja token-validoinnin käsin — kaikki sudenkuoppia joita auth-kirjasto on jo ratkaissut.

API key -tuki Home Assistantille:

- Better Auth tarjoaa API key -pluginin tai vaihtoehtoena Bearer token -toteutuksen
- Joka tapauksessa: generoidaan satunnainen token, tallennetaan hashattu versio kantaan, validoidaan `Authorization: Bearer <key>` -headerista
- Tämä on yksinkertaista toteuttaa Better Authin päälle

**Evidence**: Better Auth npm page: v1.4.19, 1.2M weekly downloads, MIT license, 691 files, 4.65MB unpacked. Lucia npm: deprecated, author message points to migration guide. Auth0 pricing: Free tier 25k MAU, Essentials $35/mo. [Source: npmjs.com/package/better-auth, npmjs.com/package/lucia, auth0.com/pricing, better-auth.com]

### Scheduled Jobs

| Approach                   | Pros                                              | Cons                                                   |
| -------------------------- | ------------------------------------------------- | ------------------------------------------------------ |
| **Railway Cron Jobs**      | No idle resource cost; Railway manages scheduling | Separate service; 5-min minimum interval; UTC-based    |
| **node-cron (in-process)** | Simple; runs in same process; no extra service    | Process must stay alive; wastes resources between runs |
| **Built-in setInterval**   | Zero dependencies                                 | Drift; no cron expression; unreliable for daily tasks  |

**Recommendation: node-cron (in-process) for MVP, Railway Cron for optimization later**

Rationale:

- For MVP, running `node-cron` inside the web server process is simplest
- The daily price fetch is a single HTTP call + DB insert — takes <2 seconds
- Having the cron in-process means one service, one deployment, one volume
- Railway Cron Jobs require a **separate service** that starts, runs, and exits — this means a separate codebase entry point or shared code with different start commands
- Railway cron minimum interval is 5 minutes, which is fine for daily tasks
- Railway cron is UTC-based; our target is ~14:00 EET = ~12:00 UTC (winter) or ~11:00 UTC (summer)

Migration path: If the app grows and we want to save resources, extract the cron into a Railway Cron Job service that shares the same codebase but runs a different entry point.

**Evidence**: Railway cron docs confirm: separate service, must exit after task, 5-min minimum, UTC-based. [Source: docs.railway.com/reference/cron-jobs]

### Recommended Coherent Stack

```
Runtime:     Node.js 22 LTS
Language:    TypeScript (strict mode)
Framework:   Hono + @hono/node-server
Database:    SQLite via better-sqlite3 (WAL mode)
Auth:        Better Auth (self-hosted, SQLite-backed)
Scheduling:  node-cron (in-process)
Build:       tsx (dev) / tsup or esbuild (prod)
```

---

## Topic 3: Railway.com + SQLite Deployment

### Does Railway Support Persistent Volumes?

**Yes.** Railway volumes are a first-class feature.

| Plan          | Volume Size Limit            | Cost           |
| ------------- | ---------------------------- | -------------- |
| Free          | 0.5 GB                       | Included       |
| Hobby ($5/mo) | 5 GB                         | $0.15/GB/month |
| Pro ($20/mo)  | 50 GB (expandable to 250 GB) | $0.15/GB/month |

**Evidence**: Railway volumes documentation confirms persistent storage with 3,000 read/write IOPS. [Source: docs.railway.com/reference/volumes]

### Volume Characteristics

- **I/O**: 3,000 IOPS read, 3,000 IOPS write — excellent for SQLite
- **Mount**: Available at runtime (not build time), specified mount path
- **Persistence**: Data survives redeploys
- **Backups**: Manual + automated (daily/weekly/monthly) with incremental pricing
- **Limitations**:
  - One volume per service
  - **No replicas** with volumes (single instance only)
  - Small downtime on redeploy (volume must unmount/remount)
  - No built-in S/FTP access
  - Docker images running as non-root need `RAILWAY_RUN_UID=0`

### Risk: Data Loss on Redeploy?

**Low risk, but non-zero downtime.**

- Data **persists** across redeploys — the volume is not ephemeral
- However, Railway prevents multiple deployments from being active simultaneously when a volume is attached
- This means **brief downtime** during redeploy (old instance stops → new instance starts → volume remounts)
- No data loss unless the volume itself fails (mitigated by backups)

**Evidence**: Railway docs state: "To prevent data corruption, we prevent multiple deployments from being active and mounted to the same service. This means that there will be a small amount of downtime when re-deploying." [Source: docs.railway.com/reference/volumes]

### Backup Strategy

Railway supports automated backups:

- **Daily**: Kept for 6 days
- **Weekly**: Kept for 1 month
- **Monthly**: Kept for 3 months
- Backups are incremental and Copy-on-Write
- Priced same as volume storage ($0.15/GB/month for incremental data)
- Manual backups limited to 50% of volume size

For a small SQLite database (<100 MB), daily backups are essentially free.

**Evidence**: Railway backup docs confirm schedule options and incremental pricing. [Source: docs.railway.com/volumes/backups]

### Cost Estimate (Hobby Plan)

| Resource            | Estimate                         | Monthly Cost    |
| ------------------- | -------------------------------- | --------------- |
| Subscription        | —                                | $5.00           |
| CPU (~0.1 vCPU avg) | Idle web server + daily cron     | ~$2.00          |
| RAM (~128 MB)       | Node.js + SQLite                 | ~$1.28          |
| Volume (1 GB)       | SQLite DB + headroom             | ~$0.15          |
| Egress (~1 GB)      | API responses for ~tens of users | ~$0.05          |
| **Total**           |                                  | **~$5–8/month** |

The Hobby plan includes $5 of resource usage. For a small app with tens of users, **total cost should be $5–7/month** (subscription + minimal overage).

**Evidence**: Railway pricing page confirms Hobby at $5/mo with $5 included usage. Resource rates: CPU $20/vCPU/mo, RAM $10/GB/mo, Volume $0.15/GB/mo, Egress $0.05/GB. [Source: docs.railway.com/pricing/plans]

### SQLite on Railway: Practical Considerations

1. **WAL mode is essential**: Enables concurrent reads while writing. Set `PRAGMA journal_mode=WAL` on startup.
2. **Mount path**: Use `/app/data` (since Nixpacks puts app in `/app`). Set `RAILWAY_VOLUME_MOUNT_PATH=/app/data`.
3. **Single instance**: No horizontal scaling with volumes. Fine for tens of users.
4. **Graceful shutdown**: Handle SIGTERM to close SQLite connection cleanly.
5. **Health check**: Expose a `/health` endpoint that queries SQLite to verify DB is accessible.

### Alternative: If SQLite on Railway Becomes Problematic

| Alternative               | Pros                                                       | Cons                                            |
| ------------------------- | ---------------------------------------------------------- | ----------------------------------------------- |
| **Turso (hosted libsql)** | No volume needed; edge replicas; free tier (500 DBs, 9 GB) | Network latency; external dependency; async API |
| **Railway Postgres**      | Managed; Railway-native; no volume needed                  | Heavier; $0 extra if within usage budget        |
| **Fly.io + LiteFS**       | SQLite replication; multi-region                           | Different platform; more complex                |

**Recommendation**: Start with SQLite on Railway volume. It's the simplest architecture. Only migrate if we hit volume limitations or need horizontal scaling (unlikely for tens of users).

### Handling the Daily Cron on Railway

Two viable approaches:

**Approach A (Recommended): In-process node-cron**

- Single service runs web server + cron scheduler
- Cron triggers at ~12:00 UTC (14:00 EET winter / 15:00 EET summer)
- Retry logic: if fetch fails, retry every 15 minutes for 2 hours
- Pro: One service, one volume, simple deployment
- Con: Pays for idle CPU between requests

**Approach B: Railway Cron Job (separate service)**

- Cron service: `0 12 * * *` (daily at 12:00 UTC)
- Starts, fetches prices, writes to shared DB, exits
- Pro: No idle cost for cron logic
- Con: Cannot share a volume between services easily; would need to call the main service's API instead of writing directly to DB

**Verdict**: Approach A is simpler and the cost difference is negligible at this scale.

---

## Assumptions (Explicit)

| #   | Assumption                                                          | Risk if Wrong                                                     |
| --- | ------------------------------------------------------------------- | ----------------------------------------------------------------- |
| A1  | Nord Pool Data Portal API will remain open and unauthenticated      | Service fails; need to evaluate alternatives at that point        |
| A2  | Railway Hobby plan is sufficient for tens of users                  | May need Pro plan ($20/mo)                                        |
| A3  | SQLite handles our read/write pattern (1 write/day, ~100 reads/day) | Extremely low risk — SQLite handles millions                      |
| A4  | Finnish VAT rate stays at 25.5%                                     | Need config update if it changes                                  |
| A5  | Nord Pool continues publishing at ~12:45 CET                        | Cron timing may need adjustment                                   |
| A6  | Better Auth works well with Hono + better-sqlite3                   | May need adapter glue code; documentation suggests native support |

## Unknowns and Research Risks

| #   | Unknown                                                         | Impact                                   | Mitigation                                                            |
| --- | --------------------------------------------------------------- | ---------------------------------------- | --------------------------------------------------------------------- |
| U1  | Nord Pool Data Portal API rate limits or access restrictions    | Service disruption if blocked            | Low polling frequency (2–3×/day), aggressive caching, add attribution |
| U2  | better-sqlite3 native addon compilation on Railway (Nixpacks)   | Build failure                            | Test early; prebuilt binaries usually work on Linux x64               |
| U3  | Railway volume behavior during platform incidents               | Potential data unavailability            | Daily backups; stateless fallback (re-fetch from API)                 |
| U4  | Exact transfer fee and margin structure for Finnish electricity | Affects total price calculation accuracy | Make transfer/margin configurable per user                            |
| U5  | Better Auth + Hono + better-sqlite3 integration maturity        | May need adapter glue code               | Documentation suggests native support; test early in implementation   |

## Evidence Sufficiency Assessment

**Confidence: HIGH for proceeding to planning.**

- Nord Pool Data Portal API source code analyzed from `pynordpool` — confirmed open, unauthenticated, and used by HA's platinum-tier official integration
- Multiple alternative data source APIs were **tested live** with real responses verified (sahkotin.fi, porssisahko.net, Elering) — available as future fallbacks if ever needed
- Railway volume and pricing documentation is **current and detailed**
- Tech stack choices are based on **verified current versions** and documentation
- Better Auth selected based on npm download data (1.2M/week), native SQLite support, and TypeScript-first design
- The main risk area is **U1: Nord Pool Data Portal API access continuity**, mitigated by low polling frequency and the fact that HA's official integration uses the same endpoint worldwide

## Most Critical Unknown

**U1: Nord Pool Data Portal API access continuity.** The Data Portal API is open and unauthenticated today, used by Home Assistant's platinum-tier integration worldwide. However, Nord Pool could restrict access at any time. Mitigation:

1. Low polling frequency (2–3×/day) to minimize load
2. Aggressive caching — serve from SQLite, not live API
3. Clear attribution to Nord Pool as data owner
4. If access is restricted, evaluate alternatives at that point (Elering, ENTSO-E, sahkotin.fi — all tested and documented above)

## Recommended Next Research Action

No further research needed before planning. The evidence is sufficient to proceed to an implementation plan.

**Note on the Data Portal API**: The discovery of the open `dataportal-api.nordpoolgroup.com` endpoint (used by HA's platinum-tier integration) means we have a reliable, authoritative data source without authentication. No fallback sources are planned — if this API becomes unavailable, we evaluate alternatives at that point rather than maintaining unused integration code.

---

## Summary of Recommendations

| Decision               | Choice                                                         | Confidence |
| ---------------------- | -------------------------------------------------------------- | :--------: |
| Primary data source    | Nord Pool Data Portal API (dataportal-api.nordpoolgroup.com)   |    High    |
| Fallback data source   | None (fail openly; re-evaluate if primary becomes unavailable) |    High    |
| Web framework          | Hono                                                           |    High    |
| Database               | better-sqlite3 (WAL mode)                                      |    High    |
| ORM                    | None (raw SQL)                                                 |   Medium   |
| Auth                   | Better Auth (self-hosted, SQLite-backed)                       |    High    |
| Scheduling             | node-cron (in-process)                                         |    High    |
| Hosting                | Railway Hobby plan + Volume                                    |    High    |
| Estimated monthly cost | $5–7                                                           |   Medium   |

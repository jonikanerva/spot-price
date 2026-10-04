# Product Vision

> What spot-price is, and what it isn't. This file owns product direction, not technology. Read the sections relevant to the task.

---

## Vision

Spot-price is a small, self-hosted backend for Home Assistant enthusiasts who want to follow Nord Pool electricity prices and run their flexible loads — sauna, EV, heating — during the cheapest hours. It turns raw day-ahead spot prices into the number you actually pay (`spot + margin + transfer + tax + VAT`) and answers the practical question "when is the cheapest window of N minutes?". On top of that it serves one light forecast of Finnish electricity prices for the days Nord Pool hasn't published yet, so an automation can look a little further ahead. It's meant to be a quiet dependency, not a dashboard you visit.

---

## Goal

Help a single household shift its flexible loads to the cheapest electricity in their Nord Pool area, through a stable, authenticated REST API that home automations can poll.

---

## Audience and environment

- **Users:** one household per instance, run by a Home Assistant enthusiast who can self-host a small service.
- **Callers:** home automations and scripts that poll the REST API with an API key. A person uses the web UI only for setup.
- **Environment:** one Node process and one PostgreSQL database on Railway. Prices cover the user's Nord Pool delivery area; the forecast covers Finland only.
- **Conditions that affect success:** Nord Pool publishes day-ahead prices once a day and can be late. Upstreams can fail. DST changes the length of a delivery day. An automation must get a clear answer in every case.

## Core Principles

- **Total price, not just spot.** Apply the user's contract terms to Nord Pool data so the API returns the cents they actually pay, not just the raw spot number. Return both.
- **The API is the product; the UI is for setup.** Features should serve an automation first. The web UI just registers an account, configures contract settings, and shows an API key.
- **Self-hosted and single-tenant.** One instance, one household. No multi-tenant machinery, no growth funnel — keep it simple to run.
- **UTC inside, local time at the edge.** Store, schedule and calculate in UTC; convert to the user's timezone only in the response. This is what keeps night-rate and DST handling correct.
- **Nord Pool is the price source.** Published prices come from the Nord Pool Data Portal. We don't blend in other price feeds or invent a price when Nord Pool is silent — the price endpoints just say "not published yet". The forecast is a clearly-labelled estimate, kept separate from real prices.
- **Simple, typed, testable math.** Price and forecast calculations are pure, inspectable functions (incl. regularized linear regression), strictly typed, and testable without a database or network.

---

## Product Shape

1. Register on the self-hosted instance (registration can be capped or closed).
2. Configure contract settings: area, margin, day/night transfer, tax, VAT, night-rate window, timezone.
3. Generate an API key in the web UI.
4. A cron fetches Nord Pool day-ahead prices into PostgreSQL; a lighter job fetches Finnish grid data (Fingrid) for the forecast.
5. Home automation calls `/api/v1/price/now`, `/today`, `/tomorrow`, `/cheapest?duration=N`, or the forecast endpoint, and acts on the response.

Caller-visible states: a typed success; "not published yet" (`available: false` or `404`) when Nord Pool has no price, never an estimate; a forecast that says the data is not good enough instead of guessing; and a typed error for bad input, a missing or wrong API key, or a rate limit.

---

## The forecast

A light, optional extra. For the days Nord Pool hasn't published yet, spot-price estimates Finnish (FI) prices from public Fingrid grid data (wind + consumption) and our own stored price history, using transparent, closed-form math — inspectable estimators such as (regularized) linear regression with hand-chosen features, not learned non-linear models, tree ensembles, or neural nets. Because it's an estimate, it lives on its own endpoint, is clearly marked as a forecast, and isn't mixed into the real-price answers or the cheapest-window decisions. If the data isn't good enough, it says so rather than guessing. Finland only, since Fingrid is the Finnish grid operator.

---

## Non-Goals & Drift Guardrails

Spot-price isn't trying to be:

- A price-comparison or contract-switching site (no provider rankings, no affiliate links).
- A multi-tenant SaaS (no teams, no billing, no admin console).
- A home-energy dashboard (no consumption tracking, solar/battery, meter readings, or usage analytics — Home Assistant already does that).
- A heavy ML / forecasting product (the forecast stays a simple, explainable, closed-form estimate — regularized linear regression at most — not a prediction engine, tree ensemble, or neural net).
- A push / notification service (consumers poll; no webhooks or alerts).
- A mobile app.

If a change pulls the product toward a consumer dashboard, a contract marketplace, or a smart-home platform, it's probably the wrong direction.

---

## Decision Filter

Lean toward yes when a change:

1. Serves an automation or script first, not a human browsing a page.
2. Keeps prices honest — real prices stay real, the forecast stays clearly an estimate.
3. Fits single-tenant self-hosting without scale or multi-tenant complexity.
4. Keeps the data footprint small — contract settings as the only personal data, UTC internally.

---

## Success Definition

It's working when the user can say:

- "I forget it's there — my sauna and EV just run at the cheap hours."
- "The number it returns matches my electricity bill."
- "The cheapest window it picks really is the cheapest."
- "I set it up once and haven't had to touch it."

---

## Persistence and Privacy Posture

| Data or capability                                                     | Purpose                                                            | Storage or recipient                                        | Retention or expiry                                                                         |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------ | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| User account (username, hashed password), sessions                     | Sign in to the setup UI (Better Auth)                              | PostgreSQL                                                  | Until the account is removed; sessions expire. Session rows never keep the IP or user agent |
| Contract settings, one row per user                                    | Turn spot price into total price                                   | PostgreSQL                                                  | Until the user changes them                                                                 |
| API keys, plaintext                                                    | Authenticate automations; the setup UI re-displays the current key | PostgreSQL                                                  | Until the user regenerates the key, which deletes the old one                               |
| Nord Pool day-ahead prices (public)                                    | Real prices and the cheapest window                                | PostgreSQL; fetched from `dataportal-api.nordpoolgroup.com` | Kept; no pruning                                                                            |
| Fingrid wind and consumption actuals (public)                          | Forecast inputs                                                    | PostgreSQL; fetched from `data.fingrid.fi`                  | `RETENTION_DAYS` (730 days)                                                                 |
| Fingrid forecast vintages (public)                                     | Forecast inputs without look-ahead                                 | PostgreSQL; fetched from `data.fingrid.fi`                  | `VINTAGE_RETENTION_DAYS` (180 days)                                                         |
| OpenWeatherMap hourly and daily forecasts for fixed FI points (public) | Future forecast inputs                                             | PostgreSQL; fetched from `api.openweathermap.org`           | `WEATHER_RETENTION_DAYS` (400 days)                                                         |
| Rate-limit counters                                                    | Abuse protection                                                   | Process memory only                                         | Lost on restart                                                                             |

- **Forbidden data flows:** household consumption, meter readings, location beyond an area code, per-user request logs, request bodies, and IPs beyond what in-memory rate limiting needs. No data goes to any recipient other than the three upstreams above.
- **Telemetry and diagnostics:** none. Process logs on Railway only; keep API keys, passwords, tokens, and personal data out of them.
- **Approved deletion policies:** the scheduled jobs prune Fingrid actuals, Fingrid forecast vintages, and weather forecasts older than their retention constants in the table above. Regenerating an API key deletes the previous key. Any other deletion or rewrite of production data needs owner approval.

## Audience & Voice

Terse and technical. Errors say what's wrong in one line (`No current price available`); no marketing copy, no emojis.

## Open Questions

- One delivery area per user, or several (e.g. a summer cottage)? Current lean: one.
- Bump to `/api/v2` on any breaking change rather than silently mutating `v1`.

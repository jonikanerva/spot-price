# Product Vision

> What spot-price is, and what it isn't. Read it before each milestone. Keep it short and honest.

---

## Vision _(REQUIRED)_

Spot-price is a small, self-hosted backend for Home Assistant enthusiasts who want to follow Nord Pool electricity prices and run their flexible loads — sauna, EV, heating — during the cheapest hours. It turns raw day-ahead spot prices into the number you actually pay (`spot + margin + transfer + tax + VAT`) and answers the practical question "when is the cheapest window of N minutes?". On top of that it serves one light forecast of Finnish electricity prices for the days Nord Pool hasn't published yet, so an automation can look a little further ahead. It's meant to be a quiet dependency, not a dashboard you visit.

---

## Goal _(REQUIRED)_

Help a single household shift its flexible loads to the cheapest electricity in their Nord Pool area, through a stable, authenticated REST API that home automations can poll.

---

## Core Principles _(REQUIRED)_

- **Total price, not just spot.** Apply the user's contract terms to Nord Pool data so the API returns the cents they actually pay, not just the raw spot number. Return both.
- **The API is the product; the UI is for setup.** Features should serve an automation first. The web UI just registers an account, configures contract settings, and shows an API key.
- **Self-hosted and single-tenant.** One instance, one household. No multi-tenant machinery, no growth funnel — keep it simple to run.
- **UTC inside, local time at the edge.** Store, schedule and calculate in UTC; convert to the user's timezone only in the response. This is what keeps night-rate and DST handling correct.
- **Nord Pool is the price source.** Published prices come from the Nord Pool Data Portal. We don't blend in other price feeds or invent a price when Nord Pool is silent — the price endpoints just say "not published yet". The forecast is a clearly-labelled estimate, kept separate from real prices.
- **Simple, typed, testable math.** Price and forecast calculations are pure functions, strictly typed, and testable without a database or network.

---

## Product Shape _(REQUIRED)_

1. Register on the self-hosted instance (registration can be capped or closed).
2. Configure contract settings: area, margin, day/night transfer, tax, VAT, night-rate window, timezone.
3. Generate an API key in the web UI.
4. A cron fetches Nord Pool day-ahead prices into PostgreSQL; a lighter job fetches Finnish grid data (Fingrid) for the forecast.
5. Home automation calls `/api/v1/price/now`, `/today`, `/tomorrow`, `/cheapest?duration=N`, or the forecast endpoint, and acts on the response.

---

## The forecast _(REQUIRED)_

A light, optional extra. For the days Nord Pool hasn't published yet, spot-price estimates Finnish (FI) prices from public Fingrid grid data (wind + consumption) and our own stored price history, using simple, transparent math — not machine learning. Because it's an estimate, it lives on its own endpoint, is clearly marked as a forecast, and isn't mixed into the real-price answers or the cheapest-window decisions. If the data isn't good enough, it says so rather than guessing. Finland only, since Fingrid is the Finnish grid operator.

---

## Non-Goals _(REQUIRED)_

Spot-price isn't trying to be:

- A price-comparison or contract-switching site (no provider rankings, no affiliate links).
- A multi-tenant SaaS (no teams, no billing, no admin console).
- A home-energy dashboard (no consumption tracking, solar/battery, meter readings, or usage analytics — Home Assistant already does that).
- A heavy ML / forecasting product (the forecast stays a simple, explainable estimate, not a prediction engine).
- A push / notification service (consumers poll; no webhooks or alerts).
- A mobile app.

---

## Decision Filter _(REQUIRED)_

Lean toward yes when a change:

1. Serves an automation or script first, not a human browsing a page.
2. Keeps prices honest — real prices stay real, the forecast stays clearly an estimate.
3. Fits single-tenant self-hosting without scale or multi-tenant complexity.
4. Keeps the data footprint small — contract settings as the only personal data, UTC internally.

If a change pulls the product toward a consumer dashboard, a contract marketplace, or a smart-home platform, it's probably the wrong direction.

---

## Success Definition _(REQUIRED)_

It's working when the user can say:

- "I forget it's there — my sauna and EV just run at the cheap hours."
- "The number it returns matches my electricity bill."
- "The cheapest window it picks really is the cheapest."
- "I set it up once and haven't had to touch it."

---

## Persistence and Privacy Posture _(REQUIRED)_

- **Stored in PostgreSQL:** user account (email, hashed password, session via Better Auth), one row of contract settings per user, hashed API keys, Nord Pool day-ahead prices, and the Finnish Fingrid wind/consumption series for the forecast. The price and grid data are public, not personal.
- **Sent off-device:** requests to Nord Pool (`dataportal-api.nordpoolgroup.com`) and Fingrid (`data.fingrid.fi`). No telemetry, analytics, or third-party tracking.
- **Not stored:** household consumption, meter readings, location beyond an area code, per-user request logs, request bodies, or IPs beyond what in-memory rate limiting needs.
- **Logs:** just process logs on Railway; keep API keys, passwords, tokens and personal data out of them.

---

## Notes & Open Questions _(OPTIONAL)_

- **Tone:** terse and technical. Errors say what's wrong in one line (`No current price available`); no marketing copy, no emojis.
- One delivery area per user, or several (e.g. a summer cottage)? Current lean: one.
- Bump to `/api/v2` on any breaking change rather than silently mutating `v1`.

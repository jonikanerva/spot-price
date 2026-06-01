# Product Vision

> The single source of truth for what spot-price _is_ and what it is _not_. Every agent reads this on every milestone. Be specific. Be opinionated.

---

## Vision _(REQUIRED)_

Spot-price gives someone running a Finnish — or wider Nord Pool area — household a small, trustworthy backend that turns raw day-ahead electricity prices into the one number they actually pay (`spot + margin + transfer + tax + VAT`) and the one answer they actually need ("when is the cheapest contiguous window of N minutes?"). Where Nord Pool has not yet published, it may additionally serve a single, clearly-labelled, deterministic short-horizon price _forecast_ — strictly secondary, never confused with a published price, never the product's reason to exist (see _The Forecast Resource_). It is built to be a quiet dependency of a home automation setup: a Home Assistant integration, a sauna timer, an EV charger — never a dashboard the user has to visit, never a service the user has to trust with anything beyond their own contract numbers.

---

## Goal _(REQUIRED)_

Help a single household run its flexible loads on the cheapest electricity available in their Nord Pool area, by exposing total prices and cheapest-window decisions as a stable, authenticated REST API.

---

## Core Principles _(REQUIRED)_

- **Total price, not spot price.**
  The product's job is to apply the user's contract terms (margin, day/night transfer, electricity tax, VAT) to Nord Pool spot data. A raw spot price is a number; a total price is a decision-ready answer. Always return both, never confuse them.

- **The API is the product. The UI is for setup.**
  Every feature must serve a machine consumer first (Home Assistant, scripts, automations). The web UI exists only to register, configure contract settings, and read an API key. If a feature only makes sense in a browser, it does not belong here.

- **Self-hosted, single-tenant by design.**
  This runs on one person's Railway instance for that one person's home. Registration is capped. There is no growth funnel, no multi-tenant isolation theatre, no admin console. Operational simplicity beats scale.

- **UTC internally, local time only at the edge.**
  All storage, scheduling, and calculation is UTC. Conversion to `Europe/Helsinki` (or whichever IANA zone the user picked) happens at the response boundary. Drift here breaks night-rate detection and DST handling — defend it.

- **One trusted upstream for published prices.**
  Published prices come from the Nord Pool Data Portal API, the same source Home Assistant's official integration uses. Do not add fallback providers, do not blend sources, do not cache "estimates". If Nord Pool has not published, the authoritative endpoints (`/now`, `/today`, `/tomorrow`, `/cheapest`) answer "not yet" — never a guess. Cheapest-window and total-price decisions are computed over published Nord Pool prices only. The single, fully-fenced exception is the non-authoritative forecast resource defined in _The Forecast Resource_; it never enters authoritative-price math.

- **Strict TypeScript, pure calculations.**
  Price math is a pure function of `(HourlyPrice, UserSettings)`. No `any`, no `unknown` as bypass, no hidden state in calculators. The calculator must be testable without a database, a clock, or a network.

---

## Product Shape _(REQUIRED)_

1. User registers an account on the self-hosted instance (capped — registration may be closed).
2. User configures their contract settings: delivery area, margin, day/night transfer fees, electricity tax, VAT, night-rate window, timezone.
3. User generates a personal API key from the web UI.
4. A daily cron (with burst polling during the Nord Pool publication window) fetches day-ahead prices for all supported areas into PostgreSQL. A separate periodic job fetches Fingrid Open Data (FI wind + consumption) for the forecast resource.
5. The user's home automation calls `/api/v1/price/now`, `/today`, `/tomorrow`, or `/cheapest?duration=N` with their bearer token and acts on the response.
6. For days Nord Pool has not yet published, an FI household may additionally call the non-authoritative `/api/v1/price/forecast` (see _The Forecast Resource_) — clearly labelled, never a published price.

---

## Non-Goals _(REQUIRED)_

The product must not become:

- **A consumer-facing price-comparison or contract-switching site.** No "find the best electricity contract" flows, no affiliate links, no provider rankings.
- **A multi-tenant SaaS.** No team accounts, no per-organisation billing, no admin tenant management. Single-tenant self-hosted is the design centre, not a step on the way to something bigger.
- **A general home-energy dashboard.** No consumption tracking, no solar/battery integration, no meter readings, no historical-usage analytics. Other tools (Home Assistant itself, energy-monitoring platforms) own that surface. (National TSO wind/consumption series used solely as forecast regressors are public grid data, not the household's consumption, and are permitted only for the forecast resource below.)
- **A forecasting / ML product.** The authoritative API reports what Nord Pool has published and computes deterministic totals and windows. No ML or AI models, no neural nets, no decision trees, no probabilistic ranges, no "AI suggested usage", no fitted-model weights artifact, no training in the request or fetch path. The _sole_ exception is the single deterministic, closed-form forecast resource fenced in _The Forecast Resource_ below — any expansion of its inputs, model, or scope requires a fresh amendment to this file.
- **A push / notification service.** Consumers poll the API on their own schedule. No webhooks, no email alerts, no "your cheapest hour is at 02:00" notifications.
- **A mobile app.** The web UI is for setup only. There is no first-party mobile client and there will not be.

---

## The Forecast Resource _(REQUIRED — the one fenced exception)_

Spot-price exposes exactly **one** non-authoritative resource that returns estimated values: the price forecast. It is the single, deliberately-fenced exception to "One trusted upstream", the forecasting Non-Goal, and Decision Filter Q2/Q4. Everything below is a hard constraint; changing any of it requires a fresh amendment to this file.

- **One resource, clearly named.** Served only from `/api/v1/price/forecast`. There is no other estimated-value endpoint, and the forecast is never returned from, blended into, cached as, or used to compute any authoritative endpoint (`/now`, `/today`, `/tomorrow`, `/cheapest`).
- **A closed set of exactly two inputs.** (1) spot-price's own already-stored Nord Pool price history — the price signal, and the regression's fit target; (2) Fingrid Open Data (the Finnish TSO) — national wind-power and consumption forecast/actual series, used **only** as regressors, never as a price. No third input, ever — not ENTSO-E, not a weather API, not a second TSO. Fingrid is named here specifically; adding any further forecast input requires amending this file.
- **A fixed, transparent mechanism — no ML.** The forecast is computed by closed-form arithmetic only: a single linear (affine) regression `price = a·(consumption − wind) + b`, a fixed per-hour-of-day bias correction, a "same weekday last week" tail extension, and explicit price-floor clips. No fitted ML/NN/tree/probabilistic model, no persisted or loaded weights artifact, no training loop in the request or fetch path. The coefficients are a small, human-readable set of scalars derived from the stored Nord Pool history.
- **Structurally non-interchangeable with a published price.** The forecast response carries a mandatory `forecast: true` discriminant and a schema deliberately distinct from the authoritative price schema, so a consumer that points at the wrong path fails loudly rather than silently trusting a guess. It also carries an explicit confidence / `degraded` signal.
- **Fail-safe, never a stale guess.** When the inputs are stale, missing, or the fit cannot be trusted, the resource returns an explicit degraded / unavailable state — never a stale or low-confidence number dressed up as a forecast. "Never a guess" stays absolute for published prices; the forecast is honest about being an estimate or it says nothing.
- **FI-only, declared as a first-class state.** Fingrid is the Finnish TSO, so the forecast exists for the `FI` bidding zone only. For any other area the resource returns a permanent, documented `forecast unavailable for this area` state — not an error that looks like a bug. Extending the forecast beyond FI requires another upstream and a fresh amendment.

---

## Guardrails for Agents _(REQUIRED)_

When making product, UX, or feature decisions:

- Do not propose features that require collecting consumption, location, or any household data beyond contract numbers. The user's tariff is the only personal data the product needs.
- Do not add visualisation features (charts, gauges, historical price browsers) beyond what is needed to verify the configuration is correct. Decisions are made by the user's automations, not by staring at graphs.
- Do not add a second upstream **price** source, a "fallback" provider, or any form of price interpolation/estimation on the authoritative endpoints. If Nord Pool has not published, respond with `available: false` or 404. The one permitted estimate is the fenced forecast resource (`/api/v1/price/forecast`); it uses Fingrid as a regressor source only, never as a price, and never touches authoritative-price math.
- Do not introduce timezones, dates, or hour arithmetic in local time inside calculators or storage. UTC is mandatory below the response boundary; only `formatDateTimeInTimeZone` at the edge.
- Do not add features that only make sense when a human is logged into the UI (e.g. price browsing across past months). The API consumer is the primary user.
- Do not weaken type safety to ship faster. No `any`, no untyped JSON pass-through, no schema drift between `api-schemas.ts` and the actual responses.

If a feature makes the product feel more like a **consumer energy dashboard**, an **electricity-contract marketplace**, or a **smart-home platform**, it is the wrong direction.

---

## Decision Filter _(REQUIRED)_

A proposed change should only be accepted if it clearly supports the core experience.

Ask:

1. Does this serve an API consumer (home automation, script, integration) as the primary user — not a human visiting a webpage?
2. Does it preserve total-price honesty: every cent the user pays accounted for, no spot/total confusion, no estimated values? (The sole documented exception is the fenced forecast resource — see _The Forecast Resource_ — which is explicitly labelled, structurally distinct from a published price, and never enters authoritative-price math.)
3. Is it compatible with single-tenant self-hosting and a capped user base — no scale assumptions, no multi-tenant complexity?
4. Does it keep the data boundary tight: only contract settings persisted, only Nord Pool as the price upstream (and only Fingrid as the forecast-regressor upstream, walled off from every authoritative endpoint), only UTC internally?

If any answer is "no", it should not be added.

---

## Success Definition _(REQUIRED)_

The product succeeds when the user feels:

- "I forget this service exists — my sauna and EV just charge at the right times."
- "The number my API returns is the number on my electricity bill."
- "I trust that 'cheapest window' actually is the cheapest window in my area and contract."
- "I configured it once a year ago and nothing has needed my attention since."

---

## Persistence and Privacy Posture _(REQUIRED)_

- **Persisted in PostgreSQL:** Better Auth user account (email, hashed password, session), one row of `user_settings` per user (margin, day/night transfer fees, tax, VAT %, night window hours, IANA timezone, delivery area), API keys (hashed), and Nord Pool day-ahead prices in 15-minute resolution per delivery area (these are public data, not user data), and — for the forecast resource only — Fingrid Open Data wind-power and consumption series (forecast + actual) for the FI bidding zone in 15-minute resolution (also public grid data, not user data).
- **Transmitted off-device:** outbound requests to `dataportal-api.nordpoolgroup.com` for day-ahead prices, and to Fingrid Open Data (`data.fingrid.fi`) for the FI wind/consumption series that feed the forecast resource. No telemetry, no analytics, no third-party tracking, no error-reporting service.
- **Never persisted:** consumption data, meter readings, household location beyond a Nord Pool area code, API-call logs tied to a user, request bodies, IP addresses beyond what is needed for rate limiting in-memory.
- **Telemetry / analytics:** none. The only observability is process logs on Railway; logs must never contain API keys, passwords, session tokens, or user-identifying information beyond a user id where strictly necessary.

---

## Audience & Voice _(OPTIONAL)_

- **Primary audience:** the operator-user — one person who self-hosts the instance for their own household, is comfortable editing a Home Assistant YAML file, and wants their automations to make the right call at 02:00 without supervision. Secondary audience: their automations themselves, which need a stable, well-typed contract.
- **Tone:** terse and technical. Error messages state what is wrong in one sentence (`No current price available`, `User settings not found`). No marketing copy, no emojis, no "Oops!" — this is plumbing, and plumbing should be quiet and exact.

---

## Open Questions _(OPTIONAL)_

- Whether to support per-user multiple delivery areas (e.g. summer cottage in a different area), or keep one area per user. Current bias: keep it one — multi-area is a complexity tax that benefits very few users.
- Whether to expose a stable schema-versioned `/api/v2` path before introducing any breaking change, or accept that `/api/v1` may evolve with deprecation notices in the OpenAPI doc. Current bias: bump the version on any breaking shape change; never silently mutate `v1`.
- Whether to extend the forecast beyond the FI bidding zone or beyond its short (~3-day) horizon. Current bias: no — FI-only, short-horizon, single closed-form model. A multi-zone or longer forecast needs another upstream and is a different product; it requires a fresh amendment to this file, not an incremental tweak.

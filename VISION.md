# Product Vision

> The single source of truth for what spot-price *is* and what it is *not*. Every agent reads this on every milestone. Be specific. Be opinionated.

---

## Vision *(REQUIRED)*

Spot-price gives someone running a Finnish — or wider Nord Pool area — household a small, trustworthy backend that turns raw day-ahead electricity prices into the one number they actually pay (`spot + margin + transfer + tax + VAT`) and the one answer they actually need ("when is the cheapest contiguous window of N minutes?"). It is built to be a quiet dependency of a home automation setup: a Home Assistant integration, a sauna timer, an EV charger — never a dashboard the user has to visit, never a service the user has to trust with anything beyond their own contract numbers.

---

## Goal *(REQUIRED)*

Help a single household run its flexible loads on the cheapest electricity available in their Nord Pool area, by exposing total prices and cheapest-window decisions as a stable, authenticated REST API.

---

## Core Principles *(REQUIRED)*

- **Total price, not spot price.**
  The product's job is to apply the user's contract terms (margin, day/night transfer, electricity tax, VAT) to Nord Pool spot data. A raw spot price is a number; a total price is a decision-ready answer. Always return both, never confuse them.

- **The API is the product. The UI is for setup.**
  Every feature must serve a machine consumer first (Home Assistant, scripts, automations). The web UI exists only to register, configure contract settings, and read an API key. If a feature only makes sense in a browser, it does not belong here.

- **Self-hosted, single-tenant by design.**
  This runs on one person's Railway instance for that one person's home. Registration is capped. There is no growth funnel, no multi-tenant isolation theatre, no admin console. Operational simplicity beats scale.

- **UTC internally, local time only at the edge.**
  All storage, scheduling, and calculation is UTC. Conversion to `Europe/Helsinki` (or whichever IANA zone the user picked) happens at the response boundary. Drift here breaks night-rate detection and DST handling — defend it.

- **One trusted upstream.**
  Prices come from the Nord Pool Data Portal API, the same source Home Assistant's official integration uses. Do not add fallback providers, do not blend sources, do not cache "estimates". If Nord Pool has not published, the answer is "not yet" — never a guess.

- **Strict TypeScript, pure calculations.**
  Price math is a pure function of `(HourlyPrice, UserSettings)`. No `any`, no `unknown` as bypass, no hidden state in calculators. The calculator must be testable without a database, a clock, or a network.

---

## Product Shape *(REQUIRED)*

1. User registers an account on the self-hosted instance (capped — registration may be closed).
2. User configures their contract settings: delivery area, margin, day/night transfer fees, electricity tax, VAT, night-rate window, timezone.
3. User generates a personal API key from the web UI.
4. A daily cron (with burst polling during the Nord Pool publication window) fetches day-ahead prices for all supported areas into PostgreSQL.
5. The user's home automation calls `/api/v1/price/now`, `/today`, `/tomorrow`, or `/cheapest?duration=N` with their bearer token and acts on the response.

---

## Non-Goals *(REQUIRED)*

The product must not become:

- **A consumer-facing price-comparison or contract-switching site.** No "find the best electricity contract" flows, no affiliate links, no provider rankings.
- **A multi-tenant SaaS.** No team accounts, no per-organisation billing, no admin tenant management. Single-tenant self-hosted is the design centre, not a step on the way to something bigger.
- **A general home-energy dashboard.** No consumption tracking, no solar/battery integration, no meter readings, no historical-usage analytics. Other tools (Home Assistant itself, energy-monitoring platforms) own that surface.
- **A forecasting / ML product.** The product reports what Nord Pool has published and computes deterministic totals and windows. No price prediction, no "AI suggested usage", no probabilistic ranges.
- **A push / notification service.** Consumers poll the API on their own schedule. No webhooks, no email alerts, no "your cheapest hour is at 02:00" notifications.
- **A mobile app.** The web UI is for setup only. There is no first-party mobile client and there will not be.

---

## Guardrails for Agents *(REQUIRED)*

When making product, UX, or feature decisions:

- Do not propose features that require collecting consumption, location, or any household data beyond contract numbers. The user's tariff is the only personal data the product needs.
- Do not add visualisation features (charts, gauges, historical price browsers) beyond what is needed to verify the configuration is correct. Decisions are made by the user's automations, not by staring at graphs.
- Do not add a second upstream price source, a "fallback" provider, or any form of price interpolation/estimation. If Nord Pool has not published, respond with `available: false` or 404.
- Do not introduce timezones, dates, or hour arithmetic in local time inside calculators or storage. UTC is mandatory below the response boundary; only `formatDateTimeInTimeZone` at the edge.
- Do not add features that only make sense when a human is logged into the UI (e.g. price browsing across past months). The API consumer is the primary user.
- Do not weaken type safety to ship faster. No `any`, no untyped JSON pass-through, no schema drift between `api-schemas.ts` and the actual responses.

If a feature makes the product feel more like a **consumer energy dashboard**, an **electricity-contract marketplace**, or a **smart-home platform**, it is the wrong direction.

---

## Decision Filter *(REQUIRED)*

A proposed change should only be accepted if it clearly supports the core experience.

Ask:

1. Does this serve an API consumer (home automation, script, integration) as the primary user — not a human visiting a webpage?
2. Does it preserve total-price honesty: every cent the user pays accounted for, no spot/total confusion, no estimated values?
3. Is it compatible with single-tenant self-hosting and a capped user base — no scale assumptions, no multi-tenant complexity?
4. Does it keep the data boundary tight: only contract settings persisted, only Nord Pool as upstream, only UTC internally?

If any answer is "no", it should not be added.

---

## Success Definition *(REQUIRED)*

The product succeeds when the user feels:

- "I forget this service exists — my sauna and EV just charge at the right times."
- "The number my API returns is the number on my electricity bill."
- "I trust that 'cheapest window' actually is the cheapest window in my area and contract."
- "I configured it once a year ago and nothing has needed my attention since."

---

## Persistence and Privacy Posture *(REQUIRED)*

- **Persisted in PostgreSQL:** Better Auth user account (email, hashed password, session), one row of `user_settings` per user (margin, day/night transfer fees, tax, VAT %, night window hours, IANA timezone, delivery area), API keys (hashed), and Nord Pool day-ahead prices in 15-minute resolution per delivery area (these are public data, not user data).
- **Transmitted off-device:** outbound requests to `dataportal-api.nordpoolgroup.com` for day-ahead prices. No telemetry, no analytics, no third-party tracking, no error-reporting service.
- **Never persisted:** consumption data, meter readings, household location beyond a Nord Pool area code, API-call logs tied to a user, request bodies, IP addresses beyond what is needed for rate limiting in-memory.
- **Telemetry / analytics:** none. The only observability is process logs on Railway; logs must never contain API keys, passwords, session tokens, or user-identifying information beyond a user id where strictly necessary.

---

## Audience & Voice *(OPTIONAL)*

- **Primary audience:** the operator-user — one person who self-hosts the instance for their own household, is comfortable editing a Home Assistant YAML file, and wants their automations to make the right call at 02:00 without supervision. Secondary audience: their automations themselves, which need a stable, well-typed contract.
- **Tone:** terse and technical. Error messages state what is wrong in one sentence (`No current price available`, `User settings not found`). No marketing copy, no emojis, no "Oops!" — this is plumbing, and plumbing should be quiet and exact.

---

## Open Questions *(OPTIONAL)*

- Whether to support per-user multiple delivery areas (e.g. summer cottage in a different area), or keep one area per user. Current bias: keep it one — multi-area is a complexity tax that benefits very few users.
- Whether to expose a stable schema-versioned `/api/v2` path before introducing any breaking change, or accept that `/api/v1` may evolve with deprecation notices in the OpenAPI doc. Current bias: bump the version on any breaking shape change; never silently mutate `v1`.
